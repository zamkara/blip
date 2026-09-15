use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use chrono::Utc;
use clap::{Parser, Subcommand};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{
    process::Command,
    sync::mpsc,
    time::{timeout, Duration},
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Parser)]
#[command(name = "blip", about = "Self-hosted webhook deploy server")]
struct Cli {
    #[arg(short, long, default_value = "blip.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: CommandKind,
}
#[derive(Subcommand)]
enum CommandKind {
    Serve,
    Validate,
    History { project: Option<String> },
}

#[derive(Clone, Deserialize)]
struct Config {
    #[serde(default = "default_bind")]
    bind: String,
    #[serde(default = "default_history")]
    history_file: PathBuf,
    projects: Vec<Project>,
}
fn default_bind() -> String {
    "0.0.0.0:8080".into()
}
fn default_history() -> PathBuf {
    "blip-history.jsonl".into()
}
#[derive(Clone, Deserialize)]
struct Project {
    name: String,
    provider: Provider,
    secret: String,
    script: PathBuf,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    track: bool,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    lock_file: Option<PathBuf>,
}
#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Gitlab,
    Github,
    Gitea,
    Codeberg,
}
#[derive(Serialize)]
struct Record {
    timestamp: String,
    project: String,
    status: String,
    duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
}
#[derive(Clone)]
struct App {
    config: Config,
    tx: mpsc::Sender<(Project, Vec<u8>)>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let cfg = load(&cli.config)?;
    match cli.command {
        CommandKind::Validate => {
            println!("configuration valid: {} project(s)", cfg.projects.len());
        }
        CommandKind::History { project } => {
            print_history(&cfg.history_file, project.as_deref()).await?
        }
        CommandKind::Serve => serve(cfg).await?,
    }
    Ok(())
}
fn load(path: &FsPath) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let cfg: Config = toml::from_str(&text).context("parse TOML")?;
    for p in &cfg.projects {
        if p.name.is_empty() || p.secret.is_empty() {
            anyhow::bail!("project name and secret are required")
        }
    }
    Ok(cfg)
}
async fn serve(cfg: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<(Project, Vec<u8>)>(128);
    let history = cfg.history_file.clone();
    tokio::spawn(async move {
        while let Some((p, body)) = rx.recv().await {
            if let Err(e) = deploy(p, body, &history).await {
                tracing::error!(%e, "deployment failed")
            }
        }
    });
    let state = App {
        config: cfg.clone(),
        tx,
    };
    let app = Router::new()
        .route("/webhook/:project", post(webhook))
        .with_state(Arc::new(state));
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    println!("blip listening on {}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
async fn webhook(
    State(state): State<Arc<App>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(project) = state
        .config
        .projects
        .iter()
        .find(|p| p.name == name)
        .cloned()
    else {
        return (StatusCode::NOT_FOUND, "unknown project");
    };
    if !verify(&project, &headers, &body) {
        return (StatusCode::UNAUTHORIZED, "invalid webhook secret");
    }
    let event = event_name(&project.provider, &headers);
    let branch = branch_name(&project.provider, &body);
    if project
        .event
        .as_deref()
        .is_some_and(|x| Some(x) != event.as_deref())
        || project
            .branch
            .as_deref()
            .is_some_and(|x| Some(x) != branch.as_deref())
    {
        return (StatusCode::NO_CONTENT, "ignored");
    }
    match state.tx.try_send((project, body.to_vec())) {
        Ok(_) => (StatusCode::ACCEPTED, "queued"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "queue full"),
    }
}
fn event_name(p: &Provider, h: &HeaderMap) -> Option<String> {
    let key = match p {
        Provider::Gitlab => "x-gitlab-event",
        Provider::Github => "x-github-event",
        _ => "x-gitea-event",
    };
    let value = h
        .get(key)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_lowercase())?;
    Some(if value == "push hook" {
        "push".into()
    } else {
        value
    })
}
fn branch_name(p: &Provider, b: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(b).ok()?;
    let r = match p {
        Provider::Github => v["ref"].as_str()?.strip_prefix("refs/heads/"),
        _ => v["ref"].as_str()?.strip_prefix("refs/heads/"),
    };
    Some(r?.to_string())
}
fn verify(p: &Project, h: &HeaderMap, body: &[u8]) -> bool {
    match p.provider {
        Provider::Gitlab => h
            .get("x-gitlab-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == p.secret),
        _ => {
            let Some(sig) = h
                .get("x-hub-signature-256")
                .or_else(|| h.get("x-gitea-signature"))
                .and_then(|v| v.to_str().ok())
            else {
                return false;
            };
            let Ok(mut mac) = HmacSha256::new_from_slice(p.secret.as_bytes()) else {
                return false;
            };
            mac.update(body);
            let expected = hex::encode(mac.finalize().into_bytes());
            sig.trim_start_matches("sha256=") == expected
        }
    }
}
async fn deploy(p: Project, _body: Vec<u8>, history: &FsPath) -> Result<()> {
    let lock = p
        .lock_file
        .clone()
        .unwrap_or_else(|| p.script.with_extension("lock"));
    let lock_guard = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock);
    if lock_guard.is_err() {
        tracing::warn!(project=%p.name, "deployment skipped: lock exists");
        return Ok(());
    }
    let start = std::time::Instant::now();
    let mut cmd = Command::new(&p.script);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let result = if let Some(sec) = p.timeout_seconds {
        timeout(Duration::from_secs(sec), cmd.output()).await??
    } else {
        cmd.output().await?
    };
    let success = result.status.success();
    let output = if p.track {
        Some(format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ))
    } else {
        None
    };
    let rec = Record {
        timestamp: Utc::now().to_rfc3339(),
        project: p.name,
        status: if success {
            "success".into()
        } else {
            "failure".into()
        },
        duration_ms: start.elapsed().as_millis(),
        exit_code: result.status.code(),
        output,
    };
    let line = serde_json::to_string(&rec)? + "\n";
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history)
        .await?
        .write_all(line.as_bytes())
        .await?;
    let _ = std::fs::remove_file(lock);
    Ok(())
}
async fn print_history(path: &FsPath, project: Option<&str>) -> Result<()> {
    if let Ok(text) = tokio::fs::read_to_string(path).await {
        for line in text.lines() {
            if project
                .map(|p| line.contains(&format!("\"project\":\"{}\"", p)))
                .unwrap_or(true)
            {
                println!("{}", line);
            }
        }
    }
    Ok(())
}
use tokio::io::AsyncWriteExt;
