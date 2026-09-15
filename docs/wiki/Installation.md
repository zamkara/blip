# Installation and service management

The single **docs/install.sh** program bootstraps Blip. It builds Rust as a non-root user and installs the binary. The installer then delegates setup to **blip project add**, **blip config validate**, and **blip service install**.

It does not create an application deployment script. Create that executable file first.

## Install

Copy the complete command:

~~~bash
bash -c 'installer="$(mktemp)" || exit 1; curl -fsSL "https://gitlab.com/almateraincubator/utilities/blip/-/raw/dev/docs/install.sh" -o "$installer" && sudo env BLIP_USER="$USER" bash "$installer"; result=$?; rm -f -- "$installer"; exit "$result"'
~~~

The wrapper explicitly invokes Bash, so it can be pasted from Bash, Fish, or another interactive shell.

The first run asks only for:

1. Project key for **/webhook/<key>**.
2. Absolute executable file path.
3. GitLab Signing token. Leave it empty only to enter a legacy Secret token.

Event and branch selection stay in GitLab.

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
| **/var/lib/blip/blip.queue.lock** | Single advisory queue lock |

The lock file appears after the first execution and may remain present. Its existence alone does not mean the queue is locked.

## Upgrade

Run the same installer command. It:

1. Refuses a dirty or mismatched source checkout.
2. Fetches and fast-forwards the selected branch.
3. Builds and replaces the binary.
4. Preserves **/etc/blip/blip.toml**.
5. Validates configuration and restarts the service.

Set **BLIP_RECONFIGURE=1** only when intentionally replacing configuration. The installer writes a timestamped backup first.

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

### Validate reports an old-schema parse error

Migrate the project array to the keyed table described in [Configuration](Configuration.md), then validate before restarting.

### Webhook returns 202 but nothing changed

**202** means queue admission. Use **blip history**, **blip logs**, and **blip queue**, then inspect the deployment script. Blip does not pull repositories or choose an output directory.

### Cloudflare returns 502

Check **systemctl is-active blip** and confirm that **127.0.0.1:8080** is listening. A connected tunnel returns 502 when it cannot reach the local origin.

## Uninstall

~~~bash
blip service uninstall
~~~

This removes only the systemd unit. Configuration, history, lock, source checkout, and binary are preserved.
