use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, ExitStatus},
};

const UNIT_NAME: &str = "blip.service";
const UNIT_PATH: &str = "/etc/systemd/system/blip.service";
const DATA_DIR: &str = "/var/lib/blip";

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn elevate_self() -> Result<()> {
    if is_root() {
        return Ok(());
    }
    let executable = std::env::current_exe().context("locate current executable")?;
    let arguments = std::env::args_os().skip(1);
    let status = Command::new("sudo")
        .arg("--")
        .arg(executable)
        .args(arguments)
        .status()
        .context("run sudo")?;
    std::process::exit(status.code().unwrap_or(1));
}

pub fn service_user(requested: Option<String>) -> Result<String> {
    let user = requested
        .or_else(|| std::env::var("SUDO_USER").ok())
        .or_else(|| std::env::var("USER").ok())
        .context("set --user to a non-root service account")?;
    if user == "root" {
        anyhow::bail!("the Blip service must not run as root");
    }
    command_success(Command::new("id").arg(&user), "find service user")?;
    Ok(user)
}

pub fn install_service(config_path: &Path, user: &str, start: bool) -> Result<()> {
    require_root()?;
    let executable = std::env::current_exe()
        .context("locate current executable")?
        .canonicalize()
        .context("resolve current executable")?;
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("resolve config {}", config_path.display()))?;
    let group = command_output("id", &["-gn", user], "find service group")?;

    command_success(
        Command::new("install").args(["-d", "-m", "0750", "-o", user, "-g", &group, DATA_DIR]),
        "create Blip data directory",
    )?;
    command_success(
        Command::new("chown")
            .arg(format!("root:{group}"))
            .arg(&config_path),
        "set config ownership",
    )?;
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o640))
        .context("set config permissions")?;

    let unit = format!(
        "[Unit]\n\
         Description=Blip webhook deployment service\n\
         After=network-online.target\n\
         Wants=network-online.target\n\n\
         [Service]\n\
         Type=simple\n\
         User={user}\n\
         Group={group}\n\
         WorkingDirectory={DATA_DIR}\n\
         ExecStart={} --config {} serve\n\
         Restart=on-failure\n\
         RestartSec=5s\n\
         UMask=0027\n\
         NoNewPrivileges=true\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        executable.display(),
        config_path.display()
    );
    let temporary = format!("{UNIT_PATH}.tmp.{}", std::process::id());
    std::fs::write(&temporary, unit).context("write temporary systemd unit")?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&temporary, UNIT_PATH).context("install systemd unit")?;

    systemctl(&["daemon-reload"], false)?;
    if start {
        systemctl(&["enable", "--now", UNIT_NAME], false)?;
    } else {
        systemctl(&["enable", UNIT_NAME], false)?;
    }
    Ok(())
}

pub fn uninstall_service() -> Result<()> {
    require_root()?;
    let _ = systemctl(&["disable", "--now", UNIT_NAME], false);
    match std::fs::remove_file(UNIT_PATH) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("remove systemd unit"),
    }
    systemctl(&["daemon-reload"], false)?;
    Ok(())
}

pub fn service_action(action: &str) -> Result<()> {
    let privileged = matches!(action, "start" | "stop" | "restart" | "enable" | "disable");
    let arguments = match action {
        "status" => vec!["status", UNIT_NAME, "--no-pager"],
        "enable" => vec!["enable", UNIT_NAME],
        "disable" => vec!["disable", UNIT_NAME],
        "start" | "stop" | "restart" => vec![action, UNIT_NAME],
        _ => anyhow::bail!("unsupported service action: {action}"),
    };
    systemctl(&arguments, privileged)
}

pub fn logs(lines: usize, follow: bool, since: Option<&str>) -> Result<()> {
    let mut arguments = vec![
        OsString::from("--unit"),
        OsString::from(UNIT_NAME),
        OsString::from("--lines"),
        OsString::from(lines.to_string()),
    ];
    if follow {
        arguments.push(OsString::from("--follow"));
    } else {
        arguments.push(OsString::from("--no-pager"));
    }
    if let Some(since) = since {
        arguments.push(OsString::from("--since"));
        arguments.push(OsString::from(since));
    }

    let status = Command::new("journalctl")
        .args(&arguments)
        .status()
        .context("run journalctl")?;
    if status.success() {
        return Ok(());
    }
    if !is_root() {
        let retry = Command::new("sudo")
            .arg("--")
            .arg("journalctl")
            .args(&arguments)
            .status()
            .context("run journalctl with sudo")?;
        ensure_success(retry, "journalctl")?;
        return Ok(());
    }
    ensure_success(status, "journalctl")
}

fn require_root() -> Result<()> {
    if !is_root() {
        anyhow::bail!("this operation requires root privileges");
    }
    Ok(())
}

fn systemctl(arguments: &[&str], privileged: bool) -> Result<()> {
    let status = if privileged && !is_root() {
        Command::new("sudo")
            .arg("--")
            .arg("systemctl")
            .args(arguments)
            .status()
    } else {
        Command::new("systemctl").args(arguments).status()
    }
    .context("run systemctl")?;
    ensure_success(status, "systemctl")
}

fn command_success(command: &mut Command, description: &str) -> Result<()> {
    let status = command.status().with_context(|| description.to_string())?;
    ensure_success(status, description)
}

fn command_output(program: &str, arguments: &[&str], description: &str) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| description.to_string())?;
    ensure_success(output.status, description)?;
    Ok(String::from_utf8(output.stdout)
        .context("command returned non-UTF-8 output")?
        .trim()
        .to_string())
}

fn ensure_success(status: ExitStatus, program: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{program} exited with {status}")
    }
}
