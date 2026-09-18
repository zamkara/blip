# Installation and service management

The single **docs/install.sh** program bootstraps Blip. It can compile from source or install pre-built release binaries, and then delegates setup to **blip project add**, **blip config validate**, and **blip service install**.

It does not create an application deployment script. Create that executable file first.

## Install

### Method 1: Source installation (default)

~~~bash
curl -fsSL "https://gitlab.com/almateraincubator/utilities/blip/-/raw/dev/docs/install.sh" | sudo bash
~~~

The pipeline explicitly starts Bash as root and works when pasted into Bash, Fish, or another interactive shell. The installer obtains the existing non-root build account from **SUDO_USER**. Standard installation therefore needs no environment variables or command arguments.

### Method 2: Pre-built binary installation (lightweight / fast setup)

~~~bash
curl -fsSL "https://gitlab.com/almateraincubator/utilities/blip/-/raw/dev/docs/install.sh" | sudo env BLIP_INSTALL_METHOD="binary" bash
~~~

The installer detects the host operating system and architecture, downloads the release tarball along with **SHA256SUMS.txt**, verifies the checksum, installs the standalone executable, and configures the systemd service without needing a local Rust toolchain or native build dependencies.

### Method 3: Cargo package manager (crates.io)

~~~bash
cargo install bliper
sudo blip service install --user "$USER"
~~~

### Initial setup prompt

The first run asks only for:

1. Project key for **/webhook/<key>**.
2. Absolute executable file path.
3. GitLab Signing token. Leave it empty only to enter a legacy Secret token.

Event and branch selection stay in GitLab.

The same command also handles installations created with the obsolete **[[projects]]** schema. It detects that schema, saves the old file as a timestamped backup, and opens the current configuration prompt. No migration flag is required.

## Non-interactive setup

~~~bash
sudo env \
  BLIP_USER="$USER" \
  BLIP_SERVICE_USER="deploy" \
  BLIP_PROJECT_KEY="example-app" \
  BLIP_SCRIPT="/srv/example-app/deploy" \
  BLIP_SIGNING_TOKEN="whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" \
  bash /path/to/install.sh
~~~

Use **BLIP_SECRET_TOKEN** instead of **BLIP_SIGNING_TOKEN** only for legacy GitLab authentication. Avoid placing real tokens in shell history on shared machines.

## Installer settings

| Variable | Default | Meaning |
| --- | --- | --- |
| **BLIP_INSTALL_METHOD** | **source** | Installation mode: **source** (compiles from Git) or **binary** (downloads pre-built release). |
| **BLIP_USER** | **SUDO_USER** | Existing non-root account used for Git and Rust. |
| **BLIP_SERVICE_USER** | **BLIP_USER** | Non-root account that runs Blip and all deployment scripts. |
| **BLIP_REPO_URL** | Official HTTPS repository | Mirror or authenticated repository override. |
| **BLIP_REPO_BRANCH** | **dev** | Blip branch to build. |
| **BLIP_SOURCE_DIR** | **/usr/local/src/blip** | Installer-managed checkout. |
| **BLIP_INSTALL_DEPENDENCIES** | **1** | Set to **0** to forbid automatic native dependency installation. |
| **BLIP_PROJECT_KEY** | prompt | Project key and endpoint segment. |
| **BLIP_SCRIPT** | prompt | Absolute executable file path. |
| **BLIP_SIGNING_TOKEN** | prompt | Preferred GitLab token. |
| **BLIP_SECRET_TOKEN** | prompt | Legacy GitLab token. |
| **BLIP_RECONFIGURE** | **0** | Set to **1** to back up and replace the existing TOML file. |

No installer setting exists for provider, event, branch, tracking, timeout, or lock path.

## Installed paths

| Path | Purpose |
| --- | --- |
| **/usr/local/bin/blip** | Release binary |
| **/usr/local/src/blip** | Installer-managed source checkout |
| **/etc/blip/blip.toml** | Central configuration |
| **/etc/systemd/system/blip.service** | systemd unit |
| **/var/lib/blip/blip-history.jsonl** | Default basic history |
| **/var/lib/blip/blip-deliveries.jsonl** | Durable delivery queue and accepted-ID journal |
| **/var/lib/blip/blip.queue.lock** | Single advisory queue lock |

The lock file appears after the first execution and may remain present. Its existence alone does not mean the queue is locked.

The system configuration may omit **history_file** or use its default relative value. Blip resolves that value to **/var/lib/blip/blip-history.jsonl** consistently for the service and CLI, regardless of the shell's current directory.

## Upgrade

After the first installation, run either form:

~~~bash
blip --upgrade
blip -U
~~~

Blip invokes the upgrade-only path from the installer-managed source checkout. It:

1. Refuses a dirty or mismatched source checkout.
2. Fetches and fast-forwards the selected branch.
3. Builds as the selected non-root build user.
4. Replaces the binary while preserving configuration and runtime data.
5. Validates **/etc/blip/blip.toml**.
6. Reinstalls the unit with its existing non-root service user and restarts the service.

The default source is **/usr/local/src/blip** and the default branch is **dev**. Existing **BLIP_SOURCE_DIR**, **BLIP_REPO_URL**, **BLIP_REPO_BRANCH**, and **BLIP_INSTALL_DEPENDENCIES** overrides are supported. Run the documented installer when the installer-owned checkout or service does not exist.

Running the full installer remains safe for repair or reconfiguration. A normal full installation now also restarts an already active service after replacing its binary.

For a full installer run, set **BLIP_RECONFIGURE=1** only when intentionally replacing configuration. The installer writes a timestamped backup first. The upgrade command never replaces configuration.

## Service commands

~~~bash
blip service status
blip service start
blip service stop
blip service restart
blip service enable
blip service disable
blip logs --lines 100
blip logs --follow
~~~

These commands may also be run as **sudo blip ...**. Without sudo, Blip requests elevation only for the operation that needs it.

Service stop and restart wait for the active deployment script to finish. Waiting queue entries are retained for the next start. The unit uses systemd **KillMode=mixed**, so the initial stop signal reaches Blip without terminating its active child process. Because execution timeout is not implemented, a script that never exits can block service stop and must be handled by the operator.

Inspect configuration, history, and queue state:

~~~bash
blip config show
blip config validate
blip project list
blip history --limit 20
blip queue
~~~

## HTTPS tunnel example

~~~yaml
tunnel: <tunnel-uuid>
credentials-file: /path/to/<tunnel-uuid>.json

ingress:
  - hostname: hooks.example.com
    service: http://127.0.0.1:8080
  - service: http_status:404
~~~

The final catch-all rule is required. See the [official Cloudflare Tunnel configuration](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/configuration-file/).

## Troubleshooting

### Cargo works normally but fails under sudo

Set **BLIP_USER** to the account that owns the Rust toolchain. The installer runs Cargo and rustup with that account's home and PATH.

### Upgrade refuses the source checkout

**blip --upgrade** refuses dirty source state, a mismatched remote, and non-fast-forward history. Inspect **/usr/local/src/blip** and preserve or remove intentional local work manually; the upgrader never resets it.

### Replace another invalid configuration

The installer migrates the obsolete **[[projects]]** shape automatically. For another invalid configuration, correct the file manually or set **BLIP_RECONFIGURE=1** to create a timestamped backup and enter the configuration again.

### Webhook returns 202 but nothing changed

**202** means queue admission. Use **blip history**, **blip logs**, and **blip queue**, then inspect the deployment script. Blip does not pull repositories or choose an output directory.

### Cloudflare returns 502

Check **systemctl is-active blip** and confirm that **127.0.0.1:8080** is listening. A connected tunnel returns 502 when it cannot reach the local origin.

## Uninstall

~~~bash
blip service uninstall
~~~

This removes only the systemd unit. Configuration, history, delivery journal, lock, source checkout, and binary are preserved.
