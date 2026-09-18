mod config;
mod runtime;
mod system;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use config::{GitlabTemplate, Project};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(name = "blip", version, about = "Self-hosted webhook deployment queue")]
struct Cli {
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    #[arg(short = 'U', long, conflicts_with = "config")]
    upgrade: bool,
    #[command(subcommand)]
    command: Option<CommandKind>,
}

#[derive(Subcommand)]
enum CommandKind {
    Serve,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    History(HistoryArgs),
    Logs(LogsArgs),
    Queue,
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Path,
    Show {
        #[arg(long)]
        show_secrets: bool,
    },
    Validate,
    Set(ConfigSetArgs),
}

#[derive(Args)]
struct ConfigSetArgs {
    #[arg(long, value_name = "ADDRESS")]
    bind: Option<String>,
    #[arg(long, value_name = "FILE")]
    history_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum ProjectCommand {
    List,
    Add(ProjectAddArgs),
    Remove {
        key: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct ProjectAddArgs {
    #[arg(long)]
    key: String,
    #[arg(long, value_name = "FILE")]
    script: PathBuf,
    #[arg(long, conflicts_with = "secret_token")]
    signing_token: Option<String>,
    #[arg(long, conflicts_with = "signing_token")]
    secret_token: Option<String>,
    #[arg(long, default_value_t = 300)]
    timestamp_tolerance_seconds: i64,
    #[arg(long)]
    replace: bool,
}

#[derive(Args)]
struct HistoryArgs {
    #[arg(long)]
    project: Option<String>,
    #[arg(long, value_parser = ["success", "failure"])]
    status: Option<String>,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Args)]
struct LogsArgs {
    #[arg(short = 'n', long, default_value_t = 100)]
    lines: usize,
    #[arg(short, long)]
    follow: bool,
    #[arg(long)]
    since: Option<String>,
}

#[derive(Subcommand)]
enum ServiceCommand {
    Install {
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        no_start: bool,
    },
    Uninstall {
        #[arg(long)]
        yes: bool,
    },
    Status,
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let config_path = config::resolve_path(cli.config.clone());

    if cli.upgrade {
        if cli.command.is_some() {
            anyhow::bail!("--upgrade cannot be combined with another command");
        }
        system::upgrade()?;
        return Ok(());
    }

    let command = cli
        .command
        .context("a command is required; use --help for available commands")?;

    match command {
        CommandKind::Serve => runtime::serve(config::load(&config_path)?).await?,
        CommandKind::Config { command } => handle_config(command, &config_path)?,
        CommandKind::Project { command } => handle_project(command, &config_path)?,
        CommandKind::History(arguments) => {
            let config = config::load(&config_path)?;
            for line in runtime::read_history(
                &config.history_file,
                arguments.project.as_deref(),
                arguments.status.as_deref(),
                arguments.limit,
            )
            .await?
            {
                println!("{line}");
            }
        }
        CommandKind::Logs(arguments) => {
            system::logs(
                arguments.lines,
                arguments.follow,
                arguments.since.as_deref(),
            )?;
        }
        CommandKind::Queue => {
            let config = config::load(&config_path)?;
            let lock = runtime::queue_lock_path(&config.history_file);
            let deliveries = runtime::delivery_file_path(&config.history_file);
            let stats = runtime::delivery_stats(&deliveries).await?;
            println!("capacity: {}", runtime::QUEUE_CAPACITY);
            println!("lock: {}", lock.display());
            println!("deliveries: {}", deliveries.display());
            println!("queued: {}", stats.queued);
            println!("running: {}", stats.running);
            println!("completed: {}", stats.completed);
            println!("state: {}", runtime::queue_state(&config.history_file)?);
        }
        CommandKind::Service { command } => handle_service(command, &config_path)?,
    }

    Ok(())
}

fn handle_config(command: ConfigCommand, path: &Path) -> Result<()> {
    match command {
        ConfigCommand::Path => println!("{}", path.display()),
        ConfigCommand::Show { show_secrets } => {
            let config = config::load(path)?;
            print!("{}", config::render(&config, show_secrets));
        }
        ConfigCommand::Validate => {
            let config = config::load(path)?;
            println!("configuration valid: {} project(s)", config.projects.len());
        }
        ConfigCommand::Set(arguments) => {
            elevate_for_system_config(path)?;
            if arguments.bind.is_none() && arguments.history_file.is_none() {
                anyhow::bail!("set at least one of --bind or --history-file");
            }
            let mut config = config::load_or_default(path)?;
            if let Some(bind) = arguments.bind {
                config.bind = bind;
            }
            if let Some(history_file) = arguments.history_file {
                config.history_file = history_file;
            }
            config::save(path, &config)?;
            println!("updated {}", path.display());
        }
    }
    Ok(())
}

fn handle_project(command: ProjectCommand, path: &Path) -> Result<()> {
    match command {
        ProjectCommand::List => {
            let config = config::load(path)?;
            for (key, project) in config.projects {
                println!("{key}\tgitlab\t{}", project.script.display());
            }
        }
        ProjectCommand::Add(mut arguments) => {
            elevate_for_system_config(path)?;
            let mut config = config::load_or_default(path)?;
            if config.projects.contains_key(&arguments.key) && !arguments.replace {
                anyhow::bail!(
                    "project {:?} already exists; pass --replace to update it",
                    arguments.key
                );
            }

            let (signing_token, secret_token) = credentials(&mut arguments)?;
            config.projects.insert(
                arguments.key.clone(),
                Project {
                    script: arguments.script,
                    gitlab: GitlabTemplate {
                        signing_token,
                        secret_token,
                        timestamp_tolerance_seconds: arguments.timestamp_tolerance_seconds,
                    },
                },
            );
            config::save(path, &config)?;
            println!("saved project {}", arguments.key);
        }
        ProjectCommand::Remove { key, yes } => {
            elevate_for_system_config(path)?;
            if !yes && !confirm(&format!("remove project {key:?}?"))? {
                println!("cancelled");
                return Ok(());
            }
            let mut config = config::load(path)?;
            if config.projects.remove(&key).is_none() {
                anyhow::bail!("unknown project: {key}");
            }
            config::save(path, &config)?;
            println!("removed project {key}");
        }
    }
    Ok(())
}

fn credentials(arguments: &mut ProjectAddArgs) -> Result<(Option<String>, Option<String>)> {
    if arguments.signing_token.is_some() || arguments.secret_token.is_some() {
        return Ok((
            arguments.signing_token.take(),
            arguments.secret_token.take(),
        ));
    }

    let signing_token =
        rpassword::prompt_password("GitLab Signing token (leave empty for Secret token): ")?;
    if !signing_token.is_empty() {
        return Ok((Some(signing_token), None));
    }
    let secret_token = rpassword::prompt_password("GitLab Secret token: ")?;
    if secret_token.is_empty() {
        anyhow::bail!("a GitLab Signing token or Secret token is required");
    }
    Ok((None, Some(secret_token)))
}

fn handle_service(command: ServiceCommand, config_path: &Path) -> Result<()> {
    match command {
        ServiceCommand::Install { user, no_start } => {
            if !system::is_root() {
                system::elevate_self()?;
            }
            config::load(config_path)?;
            let user = system::service_user(user)?;
            system::install_service(config_path, &user, !no_start)?;
            println!("installed blip.service for {user}");
        }
        ServiceCommand::Uninstall { yes } => {
            if !system::is_root() {
                system::elevate_self()?;
            }
            if !yes && !confirm("uninstall blip.service?")? {
                println!("cancelled");
                return Ok(());
            }
            system::uninstall_service()?;
            println!("uninstalled blip.service; configuration and data were preserved");
        }
        ServiceCommand::Status => system::service_action("status")?,
        ServiceCommand::Start => system::service_action("start")?,
        ServiceCommand::Stop => system::service_action("stop")?,
        ServiceCommand::Restart => system::service_action("restart")?,
        ServiceCommand::Enable => system::service_action("enable")?,
        ServiceCommand::Disable => system::service_action("disable")?,
    }
    Ok(())
}

fn elevate_for_system_config(path: &Path) -> Result<()> {
    if path.starts_with("/etc") && !system::is_root() {
        system::elevate_self()?;
    }
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_side_rules_are_rejected_from_configuration() {
        let result = toml::from_str::<config::Config>(
            r#"
                [projects.example-app]
                script = "/bin/true"
                event = "push"
                gitlab.secret_token = "test-secret"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn upgrade_short_flag_does_not_require_a_subcommand() {
        let cli = Cli::try_parse_from(["blip", "-U"]).unwrap();
        assert!(cli.upgrade);
        assert!(cli.command.is_none());
    }
}
