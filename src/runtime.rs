use crate::config::{decode_signing_token, Config, GitlabTemplate, Project};
use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use base64::Engine;
use chrono::Utc;
use fs2::FileExt;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{io::AsyncWriteExt, process::Command, sync::mpsc};

type HmacSha256 = Hmac<Sha256>;
pub const QUEUE_CAPACITY: usize = 128;
const MAX_DELIVERY_ID_LENGTH: usize = 256;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub timestamp: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    pub status: String,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct DeliveryRecord {
    accepted_at: String,
    project: String,
    delivery_id: String,
}

struct Job {
    project_key: String,
    project: Project,
    delivery_id: String,
}

#[derive(Clone)]
struct App {
    projects: Arc<BTreeMap<String, Project>>,
    tx: mpsc::Sender<Job>,
    delivery_file: PathBuf,
}

pub async fn serve(config: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Job>(QUEUE_CAPACITY);
    let history_file = config.history_file.clone();
    let queue_lock_file = queue_lock_path(&history_file);
    let delivery_file = delivery_file_path(&history_file);

    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            if let Err(error) = deploy(&job, &history_file, &queue_lock_file).await {
                tracing::error!(project = %job.project_key, delivery_id = %job.delivery_id, %error, "deployment failed");
            }
        }
    });

    let state = App {
        projects: Arc::new(config.projects),
        tx,
        delivery_file,
    };
    let app = Router::new()
        .route("/webhook/:project", post(webhook))
        .with_state(Arc::new(state));
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    println!("blip listening on {}", config.bind);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn webhook(
    State(state): State<Arc<App>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(project) = state.projects.get(&key).cloned() else {
        return (StatusCode::NOT_FOUND, "unknown project");
    };

    if !verify_gitlab(&project.gitlab, &headers, &body) {
        return (
            StatusCode::UNAUTHORIZED,
            "invalid GitLab webhook credential",
        );
    }

    let delivery_id = match gitlab_delivery_id(&headers) {
        Ok(delivery_id) => delivery_id,
        Err(message) => return (StatusCode::BAD_REQUEST, message),
    };

    let permit = match state.tx.clone().try_reserve_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return match delivery_exists(&state.delivery_file, &key, &delivery_id).await {
                Ok(true) => (StatusCode::ACCEPTED, "duplicate"),
                Ok(false) => (StatusCode::SERVICE_UNAVAILABLE, "queue full"),
                Err(error) => {
                    tracing::error!(project = %key, %delivery_id, %error, "delivery registry unavailable");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "delivery registry unavailable",
                    )
                }
            };
        }
    };

    match claim_delivery(&state.delivery_file, &key, &delivery_id).await {
        Ok(true) => {
            permit.send(Job {
                project_key: key,
                project,
                delivery_id,
            });
            (StatusCode::ACCEPTED, "queued")
        }
        Ok(false) => (StatusCode::ACCEPTED, "duplicate"),
        Err(error) => {
            tracing::error!(project = %key, %delivery_id, %error, "delivery registry unavailable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery registry unavailable",
            )
        }
    }
}

fn gitlab_delivery_id(headers: &HeaderMap) -> std::result::Result<String, &'static str> {
    let webhook_id = headers
        .get("webhook-id")
        .and_then(|value| value.to_str().ok());
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());

    let value = match (webhook_id, idempotency_key) {
        (Some(current), Some(legacy)) if current != legacy => {
            return Err("conflicting GitLab delivery IDs");
        }
        (Some(current), _) => current,
        (None, Some(legacy)) => legacy,
        (None, None) => return Err("missing GitLab delivery ID"),
    };

    if value.is_empty() || value.len() > MAX_DELIVERY_ID_LENGTH {
        return Err("invalid GitLab delivery ID");
    }
    Ok(value.to_string())
}

fn verify_gitlab(template: &GitlabTemplate, headers: &HeaderMap, body: &[u8]) -> bool {
    if let Some(signatures) = headers
        .get("webhook-signature")
        .and_then(|value| value.to_str().ok())
    {
        return verify_gitlab_signature(template, headers, body, signatures);
    }

    let (Some(expected), Some(received)) = (
        template.secret_token.as_deref(),
        headers
            .get("x-gitlab-token")
            .and_then(|value| value.to_str().ok()),
    ) else {
        return false;
    };

    constant_time_eq(received.as_bytes(), expected.as_bytes())
}

fn verify_gitlab_signature(
    template: &GitlabTemplate,
    headers: &HeaderMap,
    body: &[u8],
    received_signatures: &str,
) -> bool {
    let (Some(token), Some(message_id), Some(timestamp)) = (
        template.signing_token.as_deref(),
        headers
            .get("webhook-id")
            .and_then(|value| value.to_str().ok()),
        headers
            .get("webhook-timestamp")
            .and_then(|value| value.to_str().ok()),
    ) else {
        return false;
    };

    let Ok(timestamp_number) = timestamp.parse::<i64>() else {
        return false;
    };
    if (Utc::now().timestamp() - timestamp_number).abs() > template.timestamp_tolerance_seconds {
        return false;
    }

    let Ok(key) = decode_signing_token(token) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return false;
    };
    mac.update(message_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected = format!(
        "v1,{}",
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    );

    received_signatures
        .split_ascii_whitespace()
        .any(|candidate| constant_time_eq(candidate.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

pub fn queue_lock_path(history_file: &FsPath) -> PathBuf {
    match history_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.join("blip.queue.lock"),
        None => PathBuf::from("blip.queue.lock"),
    }
}

pub fn delivery_file_path(history_file: &FsPath) -> PathBuf {
    match history_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.join("blip-deliveries.jsonl"),
        None => PathBuf::from("blip-deliveries.jsonl"),
    }
}

async fn delivery_exists(path: &FsPath, project: &str, delivery_id: &str) -> Result<bool> {
    access_delivery_registry(path, project, delivery_id, false).await
}

async fn claim_delivery(path: &FsPath, project: &str, delivery_id: &str) -> Result<bool> {
    access_delivery_registry(path, project, delivery_id, true)
        .await
        .map(|already_exists| !already_exists)
}

async fn access_delivery_registry(
    path: &FsPath,
    project: &str,
    delivery_id: &str,
    append_when_missing: bool,
) -> Result<bool> {
    let path = path.to_path_buf();
    let project = project.to_string();
    let delivery_id = delivery_id.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("open delivery registry {}", path.display()))?;
        FileExt::lock_exclusive(&file)
            .with_context(|| format!("lock delivery registry {}", path.display()))?;

        file.seek(SeekFrom::Start(0))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        for (index, line) in contents.lines().enumerate() {
            let record = serde_json::from_str::<DeliveryRecord>(line).with_context(|| {
                format!(
                    "parse delivery registry {} line {}",
                    path.display(),
                    index + 1
                )
            })?;
            if record.project == project && record.delivery_id == delivery_id {
                FileExt::unlock(&file)?;
                return Ok(true);
            }
        }

        if append_when_missing {
            file.seek(SeekFrom::End(0))?;
            serde_json::to_writer(
                &mut file,
                &DeliveryRecord {
                    accepted_at: Utc::now().to_rfc3339(),
                    project,
                    delivery_id,
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_data()?;
        }
        FileExt::unlock(&file)?;
        Ok(false)
    })
    .await
    .context("delivery registry task failed")?
}

pub fn queue_state(history_file: &FsPath) -> Result<&'static str> {
    let path = queue_lock_path(history_file);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open queue lock {}", path.display()))?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok("idle")
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok("executing"),
        Err(error) => Err(error).with_context(|| format!("inspect queue lock {}", path.display())),
    }
}

async fn acquire_queue_lock(path: &FsPath) -> Result<File> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open queue lock {}", path.display()))?;
        FileExt::lock_exclusive(&file)
            .with_context(|| format!("acquire queue lock {}", path.display()))?;
        Ok(file)
    })
    .await
    .context("queue lock task failed")?
}

async fn deploy(job: &Job, history_file: &FsPath, queue_lock_file: &FsPath) -> Result<()> {
    let queue_lock = acquire_queue_lock(queue_lock_file).await?;
    let start = std::time::Instant::now();
    tracing::info!(project = %job.project_key, delivery_id = %job.delivery_id, script = %job.project.script.display(), "deployment started");

    let result = Command::new(&job.project.script)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await;

    let (status, exit_code, error) = match result {
        Ok(process_status) if process_status.success() => {
            ("success".to_string(), process_status.code(), None)
        }
        Ok(process_status) => (
            "failure".to_string(),
            process_status.code(),
            Some(format!("process exited with {process_status}")),
        ),
        Err(error) => ("failure".to_string(), None, Some(error.to_string())),
    };

    let record = Record {
        timestamp: Utc::now().to_rfc3339(),
        project: job.project_key.clone(),
        delivery_id: Some(job.delivery_id.clone()),
        status,
        duration_ms: start.elapsed().as_millis(),
        exit_code,
        error,
    };
    append_history(history_file, &record).await?;
    FileExt::unlock(&queue_lock).context("release queue lock")?;

    tracing::info!(
        project = %job.project_key,
        delivery_id = %job.delivery_id,
        status = %record.status,
        duration_ms = record.duration_ms,
        "deployment finished"
    );
    Ok(())
}

async fn append_history(path: &FsPath, record: &Record) -> Result<()> {
    let line = serde_json::to_string(record)? + "\n";
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open history {}", path.display()))?
        .write_all(line.as_bytes())
        .await
        .with_context(|| format!("write history {}", path.display()))?;
    Ok(())
}

pub async fn read_history(
    path: &FsPath,
    project: Option<&str>,
    status: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<String>> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read history {}", path.display()))
        }
    };

    let mut lines = text
        .lines()
        .filter_map(|line| {
            let record = serde_json::from_str::<Record>(line).ok()?;
            if project.is_some_and(|expected| record.project != expected)
                || status.is_some_and(|expected| record.status != expected)
            {
                return None;
            }
            Some(line.to_string())
        })
        .collect::<Vec<_>>();
    if let Some(limit) = limit {
        let keep_from = lines.len().saturating_sub(limit);
        lines.drain(..keep_from);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gitlab_template() -> GitlabTemplate {
        GitlabTemplate {
            signing_token: None,
            secret_token: Some("correct horse battery staple".into()),
            timestamp_tolerance_seconds: 300,
        }
    }

    #[test]
    fn secret_token_verification_uses_the_gitlab_header() {
        let template = gitlab_template();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-gitlab-token",
            "correct horse battery staple".parse().unwrap(),
        );
        assert!(verify_gitlab(&template, &headers, b"{}"));
        headers.insert("x-gitlab-token", "wrong".parse().unwrap());
        assert!(!verify_gitlab(&template, &headers, b"{}"));
    }

    #[test]
    fn signing_token_verifies_the_unmodified_body() {
        let key = b"0123456789abcdef0123456789abcdef";
        let token = format!(
            "whsec_{}",
            base64::engine::general_purpose::STANDARD.encode(key)
        );
        let template = GitlabTemplate {
            signing_token: Some(token),
            secret_token: None,
            timestamp_tolerance_seconds: 300,
        };
        let timestamp = Utc::now().timestamp().to_string();
        let message_id = "delivery-123";
        let body = br#"{"ref":"refs/heads/dev"}"#;
        let mut mac = HmacSha256::new_from_slice(key).unwrap();
        mac.update(message_id.as_bytes());
        mac.update(b".");
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = format!(
            "v1,{}",
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        );
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", message_id.parse().unwrap());
        headers.insert("webhook-timestamp", timestamp.parse().unwrap());
        headers.insert(
            "webhook-signature",
            format!("v1,invalid {signature}").parse().unwrap(),
        );

        assert!(verify_gitlab(&template, &headers, body));
        assert!(!verify_gitlab(&template, &headers, b"altered"));
    }

    #[test]
    fn queue_lock_path_is_global_to_the_history_directory() {
        assert_eq!(
            queue_lock_path(FsPath::new("/var/lib/blip/history.jsonl")),
            PathBuf::from("/var/lib/blip/blip.queue.lock")
        );
        assert_eq!(
            queue_lock_path(FsPath::new("history.jsonl")),
            PathBuf::from("blip.queue.lock")
        );
    }

    #[test]
    fn delivery_id_uses_current_and_legacy_gitlab_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "legacy-id".parse().unwrap());
        assert_eq!(gitlab_delivery_id(&headers).unwrap(), "legacy-id");

        headers.insert("webhook-id", "current-id".parse().unwrap());
        assert_eq!(
            gitlab_delivery_id(&headers),
            Err("conflicting GitLab delivery IDs")
        );

        headers.insert("idempotency-key", "current-id".parse().unwrap());
        assert_eq!(gitlab_delivery_id(&headers).unwrap(), "current-id");
    }

    #[test]
    fn delivery_file_is_derived_from_the_history_directory() {
        assert_eq!(
            delivery_file_path(FsPath::new("/var/lib/blip/history.jsonl")),
            PathBuf::from("/var/lib/blip/blip-deliveries.jsonl")
        );
        assert_eq!(
            delivery_file_path(FsPath::new("history.jsonl")),
            PathBuf::from("blip-deliveries.jsonl")
        );
    }

    #[tokio::test]
    async fn delivery_claim_survives_queued_completed_and_restart_checks() {
        let path = temporary_path("delivery-registry");

        assert!(claim_delivery(&path, "example-app", "delivery-123")
            .await
            .unwrap());
        assert!(!claim_delivery(&path, "example-app", "delivery-123")
            .await
            .unwrap());
        assert!(!claim_delivery(&path, "example-app", "delivery-123")
            .await
            .unwrap());
        assert!(claim_delivery(&path, "another-app", "delivery-123")
            .await
            .unwrap());

        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn concurrent_delivery_claim_has_one_winner() {
        let path = temporary_path("concurrent-delivery-registry");
        let first = claim_delivery(&path, "example-app", "delivery-456");
        let second = claim_delivery(&path, "example-app", "delivery-456");
        let (first, second) = tokio::join!(first, second);

        assert_ne!(first.unwrap(), second.unwrap());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn webhook_deduplicates_queued_completed_and_restarted_delivery() {
        let history_file = temporary_path("dedup-history");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        let mut projects = BTreeMap::new();
        projects.insert(
            "example-app".to_string(),
            Project {
                script: PathBuf::from("/bin/true"),
                gitlab: gitlab_template(),
            },
        );
        let projects = Arc::new(projects);
        let (tx, mut rx) = mpsc::channel(1);
        let state = Arc::new(App {
            projects: projects.clone(),
            tx,
            delivery_file: delivery_file.clone(),
        });

        assert_eq!(
            invoke_webhook(state.clone()).await,
            (StatusCode::ACCEPTED, "queued".to_string())
        );
        assert_eq!(
            invoke_webhook(state).await,
            (StatusCode::ACCEPTED, "duplicate".to_string())
        );

        let job = rx.try_recv().unwrap();
        deploy(&job, &history_file, &queue_lock_file).await.unwrap();
        let (tx, _) = mpsc::channel(1);
        let restarted = Arc::new(App {
            projects,
            tx,
            delivery_file: delivery_file.clone(),
        });
        assert_eq!(
            invoke_webhook(restarted).await,
            (StatusCode::ACCEPTED, "duplicate".to_string())
        );

        let history = read_history(&history_file, None, None, None).await.unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("\"delivery_id\":\"delivery-789\""));

        std::fs::remove_file(history_file).unwrap();
        std::fs::remove_file(delivery_file).unwrap();
        std::fs::remove_file(queue_lock_file).unwrap();
    }

    async fn invoke_webhook(state: Arc<App>) -> (StatusCode, String) {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-gitlab-token",
            "correct horse battery staple".parse().unwrap(),
        );
        headers.insert("idempotency-key", "delivery-789".parse().unwrap());
        let response = webhook(
            State(state),
            Path("example-app".to_string()),
            headers,
            Bytes::from_static(b"{}"),
        )
        .await
        .into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn temporary_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "blip-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ))
    }

    #[tokio::test]
    async fn queue_lock_excludes_a_second_owner() {
        let path = temporary_path("queue-lock-test");
        let first = acquire_queue_lock(&path).await.unwrap();
        let second = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        assert!(FileExt::try_lock_exclusive(&second).is_err());
        FileExt::unlock(&first).unwrap();
        FileExt::try_lock_exclusive(&second).unwrap();
        FileExt::unlock(&second).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
