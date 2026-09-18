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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{watch, Notify},
};

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryState {
    Queued,
    Running,
    Completed,
}

#[derive(Deserialize)]
struct StoredDeliveryRecord {
    accepted_at: String,
    #[serde(default)]
    sequence: Option<u64>,
    project: String,
    delivery_id: String,
    #[serde(default)]
    script: Option<PathBuf>,
    #[serde(default)]
    state: Option<DeliveryState>,
}

#[derive(Serialize)]
struct DeliveryEvent<'a> {
    accepted_at: &'a str,
    updated_at: String,
    sequence: u64,
    project: &'a str,
    delivery_id: &'a str,
    script: &'a FsPath,
    state: DeliveryState,
}

#[derive(Clone)]
struct Delivery {
    accepted_at: String,
    sequence: u64,
    project: String,
    delivery_id: String,
    script: Option<PathBuf>,
    state: DeliveryState,
}

struct Job {
    project_key: String,
    script: PathBuf,
    delivery_id: String,
}

#[derive(Clone)]
struct App {
    projects: Arc<BTreeMap<String, Project>>,
    wake_worker: Arc<Notify>,
    delivery_file: PathBuf,
    accepting: Arc<AtomicBool>,
}

#[derive(Debug, Eq, PartialEq)]
enum Admission {
    Queued,
    Duplicate,
    Full,
}

#[derive(Debug, Eq, PartialEq)]
enum WorkerStep {
    Processed,
    Empty,
    Shutdown,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct DeliveryStats {
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
}

pub async fn serve(config: Config) -> Result<()> {
    let history_file = config.history_file.clone();
    let queue_lock_file = queue_lock_path(&history_file);
    let delivery_file = delivery_file_path(&history_file);
    let recovered = recover_interrupted_deliveries(&delivery_file, &queue_lock_file).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "requeued interrupted deliveries");
    }
    let wake_worker = Arc::new(Notify::new());
    let accepting = Arc::new(AtomicBool::new(true));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let worker = tokio::spawn(worker_loop(
        delivery_file.clone(),
        history_file,
        queue_lock_file,
        wake_worker.clone(),
        shutdown_rx,
    ));

    let state = App {
        projects: Arc::new(config.projects),
        wake_worker,
        delivery_file,
        accepting: accepting.clone(),
    };
    let app = Router::new()
        .route("/webhook/:project", post(webhook))
        .with_state(Arc::new(state));
    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    println!("blip listening on {}", config.bind);
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(accepting.clone(), shutdown_tx.clone()))
        .await;
    accepting.store(false, Ordering::Release);
    let _ = shutdown_tx.send(true);
    worker.await.context("queue worker task failed")?;
    server?;
    Ok(())
}

async fn shutdown_signal(accepting: Arc<AtomicBool>, shutdown: watch::Sender<bool>) {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    tracing::error!(%error, "failed to install SIGTERM handler");
                    std::future::pending::<()>().await;
                    return;
                }
            };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::error!(%error, "failed to wait for shutdown signal");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to wait for shutdown signal");
    }

    accepting.store(false, Ordering::Release);
    let _ = shutdown.send(true);
    tracing::info!("shutdown requested; finishing the active deployment");
}

async fn webhook(
    State(state): State<Arc<App>>,
    Path(key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if !state.accepting.load(Ordering::Acquire) {
        return (StatusCode::SERVICE_UNAVAILABLE, "shutting down");
    }

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

    match admit_delivery(&state.delivery_file, &key, &delivery_id, &project.script).await {
        Ok(Admission::Queued) => {
            state.wake_worker.notify_one();
            (StatusCode::ACCEPTED, "queued")
        }
        Ok(Admission::Duplicate) => (StatusCode::ACCEPTED, "duplicate"),
        Ok(Admission::Full) => (StatusCode::SERVICE_UNAVAILABLE, "queue full"),
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

async fn with_delivery_registry<T, F>(path: &FsPath, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut File, &mut BTreeMap<(String, String), Delivery>) -> Result<T> + Send + 'static,
{
    let path = path.to_path_buf();
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
        let result = read_deliveries(&mut file, &path)
            .and_then(|mut deliveries| operation(&mut file, &mut deliveries));
        let unlock = FileExt::unlock(&file).context("unlock delivery registry");
        match (result, unlock) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    })
    .await
    .context("delivery registry task failed")?
}

fn read_deliveries(file: &mut File, path: &FsPath) -> Result<BTreeMap<(String, String), Delivery>> {
    file.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let mut deliveries = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let record = serde_json::from_str::<StoredDeliveryRecord>(line).with_context(|| {
            format!(
                "parse delivery registry {} line {}",
                path.display(),
                index + 1
            )
        })?;
        let delivery = Delivery {
            accepted_at: record.accepted_at,
            sequence: record.sequence.unwrap_or((index + 1) as u64),
            project: record.project,
            delivery_id: record.delivery_id,
            script: record.script,
            state: record.state.unwrap_or(DeliveryState::Completed),
        };
        deliveries.insert(
            (delivery.project.clone(), delivery.delivery_id.clone()),
            delivery,
        );
    }
    Ok(deliveries)
}

fn append_delivery_event(file: &mut File, delivery: &Delivery, state: DeliveryState) -> Result<()> {
    let script = delivery
        .script
        .as_deref()
        .context("durable delivery is missing its script path")?;
    file.seek(SeekFrom::End(0))?;
    serde_json::to_writer(
        &mut *file,
        &DeliveryEvent {
            accepted_at: &delivery.accepted_at,
            updated_at: Utc::now().to_rfc3339(),
            sequence: delivery.sequence,
            project: &delivery.project,
            delivery_id: &delivery.delivery_id,
            script,
            state,
        },
    )?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

async fn admit_delivery(
    path: &FsPath,
    project: &str,
    delivery_id: &str,
    script: &FsPath,
) -> Result<Admission> {
    let project = project.to_string();
    let delivery_id = delivery_id.to_string();
    let script = script.to_path_buf();
    with_delivery_registry(path, move |file, deliveries| {
        let key = (project.clone(), delivery_id.clone());
        if deliveries.contains_key(&key) {
            return Ok(Admission::Duplicate);
        }
        if deliveries
            .values()
            .filter(|delivery| delivery.state == DeliveryState::Queued)
            .count()
            >= QUEUE_CAPACITY
        {
            return Ok(Admission::Full);
        }

        let delivery = Delivery {
            accepted_at: Utc::now().to_rfc3339(),
            sequence: deliveries
                .values()
                .map(|delivery| delivery.sequence)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("delivery sequence overflow")?,
            project,
            delivery_id,
            script: Some(script),
            state: DeliveryState::Queued,
        };
        append_delivery_event(file, &delivery, DeliveryState::Queued)?;
        Ok(Admission::Queued)
    })
    .await
}

async fn next_queued_delivery(path: &FsPath) -> Result<Option<Delivery>> {
    with_delivery_registry(path, |_file, deliveries| {
        Ok(deliveries
            .values()
            .filter(|delivery| delivery.state == DeliveryState::Queued)
            .min_by(|left, right| {
                left.sequence
                    .cmp(&right.sequence)
                    .then_with(|| left.project.cmp(&right.project))
                    .then_with(|| left.delivery_id.cmp(&right.delivery_id))
            })
            .cloned())
    })
    .await
}

async fn begin_delivery(path: &FsPath, candidate: Delivery) -> Result<Option<Job>> {
    with_delivery_registry(path, move |file, deliveries| {
        let key = (candidate.project.clone(), candidate.delivery_id.clone());
        let Some(current) = deliveries.get(&key) else {
            return Ok(None);
        };
        if current.state != DeliveryState::Queued {
            return Ok(None);
        }
        append_delivery_event(file, current, DeliveryState::Running)?;
        Ok(Some(Job {
            project_key: current.project.clone(),
            script: current
                .script
                .clone()
                .context("durable delivery is missing its script path")?,
            delivery_id: current.delivery_id.clone(),
        }))
    })
    .await
}

async fn complete_delivery(path: &FsPath, job: &Job) -> Result<()> {
    let project = job.project_key.clone();
    let delivery_id = job.delivery_id.clone();
    with_delivery_registry(path, move |file, deliveries| {
        let key = (project, delivery_id);
        let current = deliveries
            .get(&key)
            .context("running delivery disappeared")?;
        if current.state != DeliveryState::Running {
            anyhow::bail!("delivery is not running");
        }
        append_delivery_event(file, current, DeliveryState::Completed)
    })
    .await
}

async fn recover_running_deliveries(path: &FsPath) -> Result<usize> {
    with_delivery_registry(path, |file, deliveries| {
        let interrupted = deliveries
            .values()
            .filter(|delivery| delivery.state == DeliveryState::Running)
            .cloned()
            .collect::<Vec<_>>();
        for delivery in &interrupted {
            append_delivery_event(file, delivery, DeliveryState::Queued)?;
        }
        Ok(interrupted.len())
    })
    .await
}

async fn recover_interrupted_deliveries(
    delivery_file: &FsPath,
    queue_lock_file: &FsPath,
) -> Result<usize> {
    let queue_lock = acquire_queue_lock(queue_lock_file).await?;
    let recovery = recover_running_deliveries(delivery_file).await;
    let unlock = FileExt::unlock(&queue_lock).context("release queue lock after recovery");
    match (recovery, unlock) {
        (Ok(recovered), Ok(())) => Ok(recovered),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

pub async fn delivery_stats(path: &FsPath) -> Result<DeliveryStats> {
    with_delivery_registry(path, |_file, deliveries| {
        let mut stats = DeliveryStats::default();
        for delivery in deliveries.values() {
            match delivery.state {
                DeliveryState::Queued => stats.queued += 1,
                DeliveryState::Running => stats.running += 1,
                DeliveryState::Completed => stats.completed += 1,
            }
        }
        Ok(stats)
    })
    .await
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

async fn try_acquire_queue_lock(path: &FsPath) -> Result<Option<File>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open queue lock {}", path.display()))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("acquire queue lock {}", path.display()))
            }
        }
    })
    .await
    .context("queue lock task failed")?
}

async fn acquire_queue_lock_until_shutdown(
    path: &FsPath,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Option<File>> {
    loop {
        if *shutdown.borrow() {
            return Ok(None);
        }
        if let Some(file) = try_acquire_queue_lock(path).await? {
            if *shutdown.borrow() {
                FileExt::unlock(&file).context("release queue lock during shutdown")?;
                return Ok(None);
            }
            return Ok(Some(file));
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(None);
                }
            }
        }
    }
}

async fn worker_loop(
    delivery_file: PathBuf,
    history_file: PathBuf,
    queue_lock_file: PathBuf,
    wake_worker: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }
        let notified = wake_worker.notified();
        match process_next_delivery_until_shutdown(
            &delivery_file,
            &history_file,
            &queue_lock_file,
            &mut shutdown,
        )
        .await
        {
            Ok(WorkerStep::Processed) => continue,
            Ok(WorkerStep::Shutdown) => break,
            Ok(WorkerStep::Empty) => {
                tokio::select! {
                    _ = notified => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                tracing::error!(%error, "durable queue worker failed");
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        }
    }
    tracing::info!("queue worker stopped");
}

#[cfg(test)]
async fn process_next_delivery(
    delivery_file: &FsPath,
    history_file: &FsPath,
    queue_lock_file: &FsPath,
) -> Result<bool> {
    let (_shutdown_tx, mut shutdown) = watch::channel(false);
    Ok(matches!(
        process_next_delivery_until_shutdown(
            delivery_file,
            history_file,
            queue_lock_file,
            &mut shutdown,
        )
        .await?,
        WorkerStep::Processed
    ))
}

async fn process_next_delivery_until_shutdown(
    delivery_file: &FsPath,
    history_file: &FsPath,
    queue_lock_file: &FsPath,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<WorkerStep> {
    let Some(candidate) = next_queued_delivery(delivery_file).await? else {
        return Ok(WorkerStep::Empty);
    };
    let Some(queue_lock) = acquire_queue_lock_until_shutdown(queue_lock_file, shutdown).await?
    else {
        return Ok(WorkerStep::Shutdown);
    };
    let Some(job) = begin_delivery(delivery_file, candidate).await? else {
        FileExt::unlock(&queue_lock).context("release queue lock")?;
        return Ok(WorkerStep::Processed);
    };

    let start = std::time::Instant::now();
    tracing::info!(project = %job.project_key, delivery_id = %job.delivery_id, script = %job.script.display(), "deployment started");

    let result = Command::new(&job.script)
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
    complete_delivery(delivery_file, &job).await?;
    FileExt::unlock(&queue_lock).context("release queue lock")?;

    tracing::info!(
        project = %job.project_key,
        delivery_id = %job.delivery_id,
        status = %record.status,
        duration_ms = record.duration_ms,
        "deployment finished"
    );
    Ok(WorkerStep::Processed)
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
    async fn durable_queue_preserves_fifo_and_completion() {
        let runtime_dir = temporary_directory("fifo-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);

        assert_eq!(
            admit_delivery(&delivery_file, "app", "first", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Queued
        );
        assert_eq!(
            admit_delivery(&delivery_file, "app", "second", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Queued
        );
        assert_eq!(
            next_queued_delivery(&delivery_file)
                .await
                .unwrap()
                .unwrap()
                .delivery_id,
            "first"
        );
        assert!(
            process_next_delivery(&delivery_file, &history_file, &queue_lock_file)
                .await
                .unwrap()
        );
        assert_eq!(
            next_queued_delivery(&delivery_file)
                .await
                .unwrap()
                .unwrap()
                .delivery_id,
            "second"
        );
        assert!(
            process_next_delivery(&delivery_file, &history_file, &queue_lock_file)
                .await
                .unwrap()
        );

        assert_eq!(
            delivery_stats(&delivery_file).await.unwrap(),
            DeliveryStats {
                queued: 0,
                running: 0,
                completed: 2,
            }
        );
        assert_eq!(
            admit_delivery(&delivery_file, "app", "first", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Duplicate
        );
        let history = read_history(&history_file, None, None, None).await.unwrap();
        assert!(history[0].contains("\"delivery_id\":\"first\""));
        assert!(history[1].contains("\"delivery_id\":\"second\""));
        remove_files(&[&history_file, &delivery_file, &queue_lock_file]);
        std::fs::remove_dir(runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn durable_queue_enforces_exact_waiting_capacity() {
        let path = temporary_path("queue-capacity");
        for index in 0..QUEUE_CAPACITY {
            assert_eq!(
                admit_delivery(
                    &path,
                    "app",
                    &format!("delivery-{index}"),
                    FsPath::new("/bin/true"),
                )
                .await
                .unwrap(),
                Admission::Queued
            );
        }
        assert_eq!(
            admit_delivery(&path, "app", "overflow", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Full
        );
        assert_eq!(
            admit_delivery(&path, "app", "delivery-0", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Duplicate
        );
        assert_eq!(delivery_stats(&path).await.unwrap().queued, QUEUE_CAPACITY);
        remove_files(&[&path]);
    }

    #[tokio::test]
    async fn concurrent_delivery_admission_has_one_winner() {
        let path = temporary_path("concurrent-delivery-registry");
        let first = admit_delivery(&path, "app", "delivery-456", FsPath::new("/bin/true"));
        let second = admit_delivery(&path, "app", "delivery-456", FsPath::new("/bin/true"));
        let (first, second) = tokio::join!(first, second);
        let outcomes = [first.unwrap(), second.unwrap()];

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == Admission::Queued)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == Admission::Duplicate)
                .count(),
            1
        );
        remove_files(&[&path]);
    }

    #[tokio::test]
    async fn delivery_identity_is_scoped_by_project() {
        let path = temporary_path("project-scoped-delivery");
        assert_eq!(
            admit_delivery(&path, "app-a", "same-id", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Queued
        );
        assert_eq!(
            admit_delivery(&path, "app-b", "same-id", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Queued
        );
        assert_eq!(delivery_stats(&path).await.unwrap().queued, 2);
        remove_files(&[&path]);
    }

    #[tokio::test]
    async fn malformed_delivery_registry_fails_closed() {
        let path = temporary_path("malformed-delivery-registry");
        std::fs::write(&path, "not-json\n").unwrap();

        assert!(
            admit_delivery(&path, "app", "new", FsPath::new("/bin/true"))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-json\n");
        remove_files(&[&path]);
    }

    #[tokio::test]
    async fn interrupted_running_delivery_is_recovered_after_restart() {
        let runtime_dir = temporary_directory("recovery-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        assert_eq!(
            admit_delivery(
                &delivery_file,
                "app",
                "interrupted",
                FsPath::new("/bin/true"),
            )
            .await
            .unwrap(),
            Admission::Queued
        );
        let candidate = next_queued_delivery(&delivery_file).await.unwrap().unwrap();
        assert!(begin_delivery(&delivery_file, candidate)
            .await
            .unwrap()
            .is_some());
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().running, 1);
        assert_eq!(
            recover_interrupted_deliveries(&delivery_file, &queue_lock_file)
                .await
                .unwrap(),
            1
        );
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().queued, 1);
        assert!(
            process_next_delivery(&delivery_file, &history_file, &queue_lock_file)
                .await
                .unwrap()
        );
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().completed, 1);
        remove_files(&[&history_file, &delivery_file, &queue_lock_file]);
        std::fs::remove_dir(runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn startup_recovery_waits_for_active_execution() {
        let runtime_dir = temporary_directory("recovery-lock-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        assert_eq!(
            admit_delivery(&delivery_file, "app", "running", FsPath::new("/bin/true"),)
                .await
                .unwrap(),
            Admission::Queued
        );
        let active_lock = acquire_queue_lock(&queue_lock_file).await.unwrap();
        let candidate = next_queued_delivery(&delivery_file).await.unwrap().unwrap();
        assert!(begin_delivery(&delivery_file, candidate)
            .await
            .unwrap()
            .is_some());

        let recovery_delivery_file = delivery_file.clone();
        let recovery_lock_file = queue_lock_file.clone();
        let recovery = tokio::spawn(async move {
            recover_interrupted_deliveries(&recovery_delivery_file, &recovery_lock_file).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!recovery.is_finished());

        let job = Job {
            project_key: "app".to_string(),
            script: PathBuf::from("/bin/true"),
            delivery_id: "running".to_string(),
        };
        complete_delivery(&delivery_file, &job).await.unwrap();
        FileExt::unlock(&active_lock).unwrap();
        assert_eq!(recovery.await.unwrap().unwrap(), 0);
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().completed, 1);

        remove_files(&[&delivery_file, &queue_lock_file]);
        std::fs::remove_dir(runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn legacy_delivery_registry_entries_remain_deduplicated() {
        let path = temporary_path("legacy-delivery-registry");
        std::fs::write(
            &path,
            "{\"accepted_at\":\"2026-09-17T00:00:00Z\",\"project\":\"app\",\"delivery_id\":\"legacy\"}\n",
        )
        .unwrap();

        assert_eq!(delivery_stats(&path).await.unwrap().completed, 1);
        assert_eq!(
            admit_delivery(&path, "app", "legacy", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Duplicate
        );
        remove_files(&[&path]);
    }

    #[tokio::test]
    async fn webhook_deduplication_survives_service_reconstruction() {
        let delivery_file = temporary_path("webhook-durable-queue");
        let mut projects = BTreeMap::new();
        projects.insert(
            "example-app".to_string(),
            Project {
                script: PathBuf::from("/bin/true"),
                gitlab: gitlab_template(),
            },
        );
        let projects = Arc::new(projects);
        let state = Arc::new(App {
            projects: projects.clone(),
            wake_worker: Arc::new(Notify::new()),
            delivery_file: delivery_file.clone(),
            accepting: Arc::new(AtomicBool::new(true)),
        });
        assert_eq!(
            invoke_webhook(state.clone()).await,
            (StatusCode::ACCEPTED, "queued".to_string())
        );
        assert_eq!(
            invoke_webhook(state).await,
            (StatusCode::ACCEPTED, "duplicate".to_string())
        );
        let restarted = Arc::new(App {
            projects,
            wake_worker: Arc::new(Notify::new()),
            delivery_file: delivery_file.clone(),
            accepting: Arc::new(AtomicBool::new(true)),
        });
        assert_eq!(
            invoke_webhook(restarted).await,
            (StatusCode::ACCEPTED, "duplicate".to_string())
        );
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().queued, 1);
        remove_files(&[&delivery_file]);
    }

    #[tokio::test]
    async fn webhook_admission_closes_during_shutdown() {
        let delivery_file = temporary_path("shutdown-admission");
        let mut projects = BTreeMap::new();
        projects.insert(
            "example-app".to_string(),
            Project {
                script: PathBuf::from("/bin/true"),
                gitlab: gitlab_template(),
            },
        );
        let state = Arc::new(App {
            projects: Arc::new(projects),
            wake_worker: Arc::new(Notify::new()),
            delivery_file: delivery_file.clone(),
            accepting: Arc::new(AtomicBool::new(false)),
        });

        assert_eq!(
            invoke_webhook(state).await,
            (StatusCode::SERVICE_UNAVAILABLE, "shutting down".to_string())
        );
        assert!(!delivery_file.exists());
    }

    #[tokio::test]
    async fn graceful_shutdown_finishes_active_job_and_preserves_waiting_job() {
        use std::os::unix::fs::PermissionsExt;

        let runtime_dir = temporary_directory("graceful-shutdown-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        let script = runtime_dir.join("deploy");
        let started = runtime_dir.join("started");
        let release = runtime_dir.join("release");
        let finished = runtime_dir.join("finished");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\ntouch '{}'\n",
                started.display(),
                release.display(),
                finished.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            admit_delivery(&delivery_file, "app", "first", &script)
                .await
                .unwrap(),
            Admission::Queued
        );
        assert_eq!(
            admit_delivery(&delivery_file, "app", "second", &script)
                .await
                .unwrap(),
            Admission::Queued
        );

        let wake_worker = Arc::new(Notify::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let worker = tokio::spawn(worker_loop(
            delivery_file.clone(),
            history_file.clone(),
            queue_lock_file.clone(),
            wake_worker,
            shutdown_rx,
        ));
        for _ in 0..200 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.exists());

        shutdown_tx.send(true).unwrap();
        std::fs::write(&release, b"").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();

        assert!(finished.exists());
        assert_eq!(
            delivery_stats(&delivery_file).await.unwrap(),
            DeliveryStats {
                queued: 1,
                running: 0,
                completed: 1,
            }
        );
        let history = read_history(&history_file, None, None, None).await.unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("\"delivery_id\":\"first\""));

        remove_files(&[
            &history_file,
            &delivery_file,
            &queue_lock_file,
            &script,
            &started,
            &release,
            &finished,
        ]);
        std::fs::remove_dir(runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn graceful_shutdown_stops_an_idle_worker() {
        let runtime_dir = temporary_directory("idle-shutdown-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let worker = tokio::spawn(worker_loop(
            delivery_file.clone(),
            history_file,
            queue_lock_file.clone(),
            Arc::new(Notify::new()),
            shutdown_rx,
        ));

        tokio::task::yield_now().await;
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();

        remove_files(&[&delivery_file, &queue_lock_file]);
        std::fs::remove_dir(runtime_dir).unwrap();
    }

    #[tokio::test]
    async fn graceful_shutdown_interrupts_global_lock_wait() {
        let runtime_dir = temporary_directory("lock-wait-shutdown-runtime");
        let history_file = runtime_dir.join("blip-history.jsonl");
        let delivery_file = delivery_file_path(&history_file);
        let queue_lock_file = queue_lock_path(&history_file);
        assert_eq!(
            admit_delivery(&delivery_file, "app", "waiting", FsPath::new("/bin/true"))
                .await
                .unwrap(),
            Admission::Queued
        );
        let active_lock = acquire_queue_lock(&queue_lock_file).await.unwrap();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let worker_delivery_file = delivery_file.clone();
        let worker_history_file = history_file.clone();
        let worker_lock_file = queue_lock_file.clone();
        let worker = tokio::spawn(async move {
            process_next_delivery_until_shutdown(
                &worker_delivery_file,
                &worker_history_file,
                &worker_lock_file,
                &mut shutdown_rx,
            )
            .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        shutdown_tx.send(true).unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WorkerStep::Shutdown
        );
        assert_eq!(delivery_stats(&delivery_file).await.unwrap().queued, 1);

        FileExt::unlock(&active_lock).unwrap();
        remove_files(&[&delivery_file, &queue_lock_file]);
        std::fs::remove_dir(runtime_dir).unwrap();
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

    fn temporary_directory(label: &str) -> PathBuf {
        let path = temporary_path(label);
        std::fs::create_dir(&path).unwrap();
        path
    }

    fn remove_files(paths: &[&FsPath]) {
        for path in paths {
            if let Err(error) = std::fs::remove_file(path) {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            }
        }
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
