use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::OpenOptions,
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_history")]
    pub history_file: PathBuf,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            history_file: default_history(),
            projects: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub script: PathBuf,
    #[serde(default)]
    pub gitlab: Option<GitlabTemplate>,
    #[serde(default)]
    pub github: Option<GithubTemplate>,
    #[serde(default)]
    pub gitea: Option<GiteaTemplate>,
    #[serde(default)]
    pub codeberg: Option<CodebergTemplate>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitlabTemplate {
    #[serde(default)]
    pub signing_token: Option<String>,
    #[serde(default)]
    pub secret_token: Option<String>,
    #[serde(default = "default_timestamp_tolerance")]
    pub timestamp_tolerance_seconds: i64,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubTemplate {
    pub secret: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GiteaTemplate {
    pub secret: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodebergTemplate {
    pub secret: String,
}

impl Project {
    pub fn provider_name(&self) -> &'static str {
        if self.gitlab.is_some() {
            "gitlab"
        } else if self.github.is_some() {
            "github"
        } else if self.gitea.is_some() {
            "gitea"
        } else if self.codeberg.is_some() {
            "codeberg"
        } else {
            "unconfigured"
        }
    }
}

pub fn default_bind() -> String {
    "127.0.0.1:8080".into()
}

pub fn default_history() -> PathBuf {
    "blip-history.jsonl".into()
}

pub fn default_timestamp_tolerance() -> i64 {
    300
}

pub fn resolve_path(requested: Option<PathBuf>) -> PathBuf {
    requested.unwrap_or_else(|| {
        let system = PathBuf::from("/etc/blip/blip.toml");
        if system.exists() {
            system
        } else {
            PathBuf::from("blip.toml")
        }
    })
}

pub fn load(path: &Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let mut config: Config = toml::from_str(&text).context("parse TOML")?;
    config.history_file = resolve_history_file(path, &config.history_file);
    validate(&config)?;
    Ok(config)
}

pub fn load_or_default(path: &Path) -> Result<Config> {
    if path.exists() {
        load(path)
    } else {
        let mut config = Config::default();
        config.history_file = resolve_history_file(path, &config.history_file);
        Ok(config)
    }
}

fn resolve_history_file(config_path: &Path, history_file: &Path) -> PathBuf {
    if history_file.is_absolute() {
        return history_file.to_path_buf();
    }
    if config_path == Path::new("/etc/blip/blip.toml") {
        return Path::new("/var/lib/blip").join(history_file);
    }
    match config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.join(history_file),
        None => history_file.to_path_buf(),
    }
}

pub fn validate(config: &Config) -> Result<()> {
    if config.bind.trim().is_empty() {
        anyhow::bail!("bind cannot be empty");
    }
    if config.history_file.as_os_str().is_empty() {
        anyhow::bail!("history_file cannot be empty");
    }

    for (key, project) in &config.projects {
        if !valid_project_key(key) {
            anyhow::bail!(
                "project key {key:?} must contain only letters, digits, dots, underscores, or hyphens"
            );
        }
        if !project.script.is_absolute() {
            anyhow::bail!("project {key:?} script must be an absolute file path");
        }

        let metadata = std::fs::metadata(&project.script)
            .with_context(|| format!("project {key:?} script {}", project.script.display()))?;
        if !metadata.is_file() {
            anyhow::bail!("project {key:?} script must be a file");
        }
        if metadata.permissions().mode() & 0o111 == 0 {
            anyhow::bail!("project {key:?} script is not executable");
        }

        let provider_count = [
            project.gitlab.is_some(),
            project.github.is_some(),
            project.gitea.is_some(),
            project.codeberg.is_some(),
        ]
        .into_iter()
        .filter(|configured| *configured)
        .count();
        if provider_count != 1 {
            anyhow::bail!("project {key:?} must configure exactly one provider template");
        }

        if let Some(template) = &project.gitlab {
            if template.timestamp_tolerance_seconds <= 0 {
                anyhow::bail!("project {key:?} GitLab timestamp tolerance must be positive");
            }
            if template.secret_token.as_deref().is_some_and(str::is_empty) {
                anyhow::bail!("project {key:?} GitLab Secret token cannot be empty");
            }
            match template.signing_token.as_deref() {
                Some(token) => {
                    decode_signing_token(token)
                        .with_context(|| format!("project {key:?} GitLab Signing token"))?;
                }
                None if template.secret_token.is_none() => {
                    anyhow::bail!(
                        "project {key:?} requires a GitLab Signing token or Secret token"
                    );
                }
                None => {}
            }
        }
        for (provider, secret) in [
            ("GitHub", project.github.as_ref().map(|value| &value.secret)),
            ("Gitea", project.gitea.as_ref().map(|value| &value.secret)),
            (
                "Codeberg",
                project.codeberg.as_ref().map(|value| &value.secret),
            ),
        ] {
            if secret.is_some_and(|value| value.is_empty()) {
                anyhow::bail!("project {key:?} {provider} secret cannot be empty");
            }
        }
    }

    Ok(())
}

pub fn valid_project_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn decode_signing_token(token: &str) -> Result<Vec<u8>> {
    let encoded = token
        .strip_prefix("whsec_")
        .context("must start with whsec_")?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("must contain valid base64 after whsec_")?;
    if key.is_empty() {
        anyhow::bail!("must contain a non-empty key");
    }
    Ok(key)
}

pub fn render(config: &Config, reveal_secrets: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!("bind = \"{}\"\n", escape(&config.bind)));
    output.push_str(&format!(
        "history_file = \"{}\"\n",
        escape(&config.history_file.to_string_lossy())
    ));

    for (key, project) in &config.projects {
        output.push_str(&format!("\n[projects.{key}]\n"));
        output.push_str(&format!(
            "script = \"{}\"\n",
            escape(&project.script.to_string_lossy())
        ));
        if let Some(template) = &project.gitlab {
            if let Some(token) = &template.signing_token {
                let value = if reveal_secrets { token } else { "<redacted>" };
                output.push_str(&format!("gitlab.signing_token = \"{}\"\n", escape(value)));
            }
            if let Some(token) = &template.secret_token {
                let value = if reveal_secrets { token } else { "<redacted>" };
                output.push_str(&format!("gitlab.secret_token = \"{}\"\n", escape(value)));
            }
            if template.timestamp_tolerance_seconds != default_timestamp_tolerance() {
                output.push_str(&format!(
                    "gitlab.timestamp_tolerance_seconds = {}\n",
                    template.timestamp_tolerance_seconds
                ));
            }
        }
        for (provider, secret) in [
            ("github", project.github.as_ref().map(|value| &value.secret)),
            ("gitea", project.gitea.as_ref().map(|value| &value.secret)),
            (
                "codeberg",
                project.codeberg.as_ref().map(|value| &value.secret),
            ),
        ] {
            if let Some(secret) = secret {
                let value = if reveal_secrets { secret } else { "<redacted>" };
                output.push_str(&format!("{provider}.secret = \"{}\"\n", escape(value)));
            }
        }
    }
    output
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    validate(config)?;
    let parent = path.parent().filter(|value| !value.as_os_str().is_empty());
    if let Some(parent) = parent {
        if !parent.exists() {
            anyhow::bail!("config directory does not exist: {}", parent.display());
        }
    }

    let existing = std::fs::metadata(path).ok();
    let mode = existing
        .as_ref()
        .map(|metadata| metadata.permissions().mode())
        .unwrap_or(0o600);
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .with_context(|| format!("create temporary config {}", temporary.display()))?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.write_all(render(config, true).as_bytes())?;
        file.sync_all()?;
        if unsafe { libc::geteuid() } == 0 {
            if let Some(metadata) = &existing {
                let path_bytes = CString::new(temporary.as_os_str().as_bytes())?;
                let result =
                    unsafe { libc::chown(path_bytes.as_ptr(), metadata.uid(), metadata.gid()) };
                if result != 0 {
                    return Err(std::io::Error::last_os_error())
                        .context("preserve config ownership");
                }
            }
        }
        std::fs::rename(&temporary, path)
            .with_context(|| format!("replace config {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> Config {
        let mut config = Config::default();
        config.projects.insert(
            "example-app".into(),
            Project {
                script: PathBuf::from("/bin/true"),
                gitlab: Some(GitlabTemplate {
                    signing_token: None,
                    secret_token: Some("test-secret".into()),
                    timestamp_tolerance_seconds: 300,
                }),
                github: None,
                gitea: None,
                codeberg: None,
            },
        );
        config
    }

    #[test]
    fn rendering_writes_the_project_key_once() {
        let rendered = render(&sample_config(), true);
        assert_eq!(rendered.matches("[projects.example-app]").count(), 1);
        assert!(!rendered.contains("[projects.example-app.gitlab]"));
        assert!(rendered.contains("gitlab.secret_token = \"test-secret\""));
    }

    #[test]
    fn rendering_redacts_credentials_by_default() {
        let rendered = render(&sample_config(), false);
        assert!(!rendered.contains("test-secret"));
        assert!(rendered.contains("gitlab.secret_token = \"<redacted>\""));
    }

    #[test]
    fn saved_config_round_trips() {
        let path = std::env::temp_dir().join(format!(
            "blip-config-test-{}-{}.toml",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        save(&path, &sample_config()).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.projects.contains_key("example-app"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn history_path_is_stable_for_system_and_local_configs() {
        assert_eq!(
            resolve_history_file(
                Path::new("/etc/blip/blip.toml"),
                Path::new("blip-history.jsonl"),
            ),
            PathBuf::from("/var/lib/blip/blip-history.jsonl")
        );
        assert_eq!(
            resolve_history_file(
                Path::new("/srv/blip/config/blip.toml"),
                Path::new("runtime/history.jsonl"),
            ),
            PathBuf::from("/srv/blip/config/runtime/history.jsonl")
        );
        assert_eq!(
            resolve_history_file(
                Path::new("/etc/blip/blip.toml"),
                Path::new("/data/blip/history.jsonl"),
            ),
            PathBuf::from("/data/blip/history.jsonl")
        );
    }

    #[test]
    fn projects_require_exactly_one_provider_template() {
        let missing = toml::from_str::<Config>(
            r#"
            [projects.app]
            script = "/bin/true"
            "#,
        )
        .unwrap();
        assert!(validate(&missing).is_err());

        let multiple = toml::from_str::<Config>(
            r#"
            [projects.app]
            script = "/bin/true"
            github.secret = "one"
            gitea.secret = "two"
            "#,
        )
        .unwrap();
        assert!(validate(&multiple).is_err());
    }

    #[test]
    fn provider_templates_round_trip_and_redact_secrets() {
        let config = toml::from_str::<Config>(
            r#"
            [projects.github]
            script = "/bin/true"
            github.secret = "github-secret"

            [projects.gitea]
            script = "/bin/true"
            gitea.secret = "gitea-secret"

            [projects.codeberg]
            script = "/bin/true"
            codeberg.secret = "codeberg-secret"
            "#,
        )
        .unwrap();
        validate(&config).unwrap();
        let redacted = render(&config, false);
        assert!(!redacted.contains("github-secret"));
        assert!(!redacted.contains("gitea-secret"));
        assert!(!redacted.contains("codeberg-secret"));
        assert_eq!(redacted.matches("<redacted>").count(), 3);
        let revealed = render(&config, true);
        let reparsed = toml::from_str::<Config>(&revealed).unwrap();
        validate(&reparsed).unwrap();
    }
}
