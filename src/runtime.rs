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
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{io::AsyncWriteExt, process::Command, sync::mpsc};

type HmacSha256 = Hmac<Sha256>;
pub const QUEUE_CAPACITY: usize = 128;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub timestamp: String,
    pub project: String,
    pub status: String,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone)]
struct App {
    projects: Arc<BTreeMap<String, Project>>,
    tx: mpsc::Sender<(String, Project)>,
}

pub async fn serve(config: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<(String, Project)>(QUEUE_CAPACITY);
    let history_file = config.history_file.clone();
    let queue_lock_file = queue_lock_path(&history_file);

    tokio::spawn(async move {
        while let Some((key, project)) = rx.recv().await {
            if let Err(error) = deploy(&key, &project, &history_file, &queue_lock_file).await {
                tracing::error!(project = %key, %error, "deployment failed");
            }
        }
    });

    let state = App {
        projects: Arc::new(config.projects),
        tx,
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

    match state.tx.try_send((key, project)) {
        Ok(()) => (StatusCode::ACCEPTED, "queued"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "queue full"),
    }
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

async fn deploy(
    project_key: &str,
    project: &Project,
    history_file: &FsPath,
    queue_lock_file: &FsPath,
) -> Result<()> {
    let queue_lock = acquire_queue_lock(queue_lock_file).await?;
    let start = std::time::Instant::now();
    tracing::info!(project = %project_key, script = %project.script.display(), "deployment started");

    let result = Command::new(&project.script)
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
        project: project_key.to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        exit_code,
        error,
    };
    append_history(history_file, &record).await?;
    FileExt::unlock(&queue_lock).context("release queue lock")?;

    tracing::info!(
        project = %project_key,
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

    #[tokio::test]
    async fn queue_lock_excludes_a_second_owner() {
        let path = std::env::temp_dir().join(format!(
            "blip-queue-lock-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
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
