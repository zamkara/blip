# Blip

Blip is a lightweight, self-hosted webhook deployment server written in Rust. It receives authenticated webhook events from Git hosting providers and runs a configured executable locally. It is designed for servers hosting many applications where a complete hosted CI/CD runner would add unnecessary cost and complexity.

## Current status

This repository contains the Phase 1 MVP:

- HTTP webhook server with a centralized TOML configuration.
- GitLab, GitHub, Gitea, and Codeberg provider support.
- Per-project event and branch filters.
- GitLab Signing token verification using the Standard Webhooks format, including timestamp replay protection.
- Legacy GitLab Secret token fallback.
- Bounded queue with serial execution and lock-file overlap prevention.
- Fire-and-forget execution by default, with optional output, exit status, and timeout tracking.
- JSON Lines deploy history queryable through the CLI.
- Native systemd deployment.

REST management API, notifications, and a dashboard are intentionally deferred.

## Deployment flow

The intended GitLab flow is:

```text
feature branch → merge request → dev → GitLab Push Hook → Blip queue → deploy executable
```

For the self-build test environment, the operational files live outside this repository. The external deploy executable updates an isolated checkout, runs the test suite, and writes the release result to `/home/zam/Desktop/blip-build`. This repository itself does not contain production secrets, webhook credentials, or operational installer state.

## Webhook endpoint

Each configured project is exposed at:

```text
POST /webhook/<project-name>
```

The server responds with `202 Accepted` after authentication and rule matching succeed. Non-matching events return `204 No Content`; unknown projects return `404`; invalid authentication returns `401`.

## Execution behavior

Deployments are placed in a bounded queue and processed by one worker. A lock file prevents overlap for the configured project. With tracking disabled, the executable is still launched but its output is not recorded. With tracking enabled, Blip records status, exit code, duration, and combined output in JSONL history. A configured timeout limits tracked execution.

## Configuration rules

Project names identify webhook paths and must be unique. `event` and `branch` are optional filters. For GitLab push events, configure `event = "push"` and use the branch name such as `dev`; GitLab's `Push Hook` header is normalized by Blip. A project must define either the provider-specific signing credential or the legacy secret credential.

## Build

Requirements: Rust stable and Cargo.

```sh
cargo fmt --check
cargo test
cargo build --release
```

The binary is created at `target/release/blip`.

## Configuration

Copy `blip.example.toml` to a private local file and update the values:

```toml
bind = "127.0.0.1:8080"
history_file = "./blip-history.jsonl"

[[projects]]
name = "my-app"
provider = "gitlab"
signing_token = "whsec_..."
script = "/srv/my-app/deploy"
event = "push"
branch = "develop"
track = true
timeout_seconds = 1800
lock_file = "/srv/my-app/.blip.lock"
```

At least one of `signing_token` or the legacy `secret` must be configured. Keep local configuration and generated history out of Git.

## CLI

```sh
./target/release/blip --config blip.toml validate
./target/release/blip --config blip.toml serve
./target/release/blip --config blip.toml history
./target/release/blip --config blip.toml history my-app
```

## GitLab webhook

For a GitLab project webhook, use the endpoint `/webhook/<project-name>`, enable **Push events**, and filter the branch in Blip configuration. Signing token authentication is preferred. GitLab sends `webhook-id`, `webhook-timestamp`, and `webhook-signature`; Blip verifies the HMAC-SHA256 signature over `id.timestamp.raw_body` and rejects stale timestamps. The legacy Secret token is sent through `X-Gitlab-Token` and may be used as a fallback.

## systemd

Install the binary and configure a systemd service whose `ExecStart` runs `blip --config /etc/blip/blip.toml serve`. The service should run with the least-privileged account that can read the deploy script and write the configured history file.

## Security

- Use HTTPS, normally through a reverse proxy or Cloudflare Tunnel.
- Treat signing tokens, secret tokens, and provider credentials as confidential.
- Keep the deploy executable and its working directories restricted.
- Use a per-project lock file to avoid overlapping deployments.
- Review tracked command output because it can contain sensitive data.

## License

MIT
