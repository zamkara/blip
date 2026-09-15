# Project Brief: Blip

## Overview
Blip is a self-hosted CI/CD webhook server. It replaces manual SSH-pull-and-deploy workflows and avoids the free-tier CI/CD minute limits (e.g. GitLab's 400 min/month) when running many apps on a single server. It listens for git hosting provider webhooks (push/merge events) and triggers local deploy scripts accordingly.

## Problem Statement
The owner runs many applications on a personal server. Currently, deployment is fully manual: after every push/merge, they SSH into the server, pull the latest code, and run a shell deploy script by hand. Using a hosted CI/CD service (GitLab CI/CD, GitHub Actions, Blacksmith, etc.) is an option, but free-tier compute minutes (e.g. GitLab's 400 min/month) are far too limited across many projects, and paying for CI/CD minutes for simple deploy triggers is wasteful. The insight: since a webhook payload arriving at a server is enough to know "a deploy should happen," a lightweight self-hosted webhook listener that just runs the existing deploy script is a cheaper, simpler dapur-mandiri alternative to full CI/CD infrastructure (e.g. self-hosted Jenkins).

## Core Concept
Install the Blip binary on the server as a long-running systemd service. Register a webhook in the git hosting provider (GitLab first) pointing to Blip's HTTP endpoint. On every matching event (push, merge, configurable), Blip validates the payload and runs the associated deploy script already used in the current manual workflow — no compute-minute billing, no external CI/CD dependency.

## Tech Stack
- **Language:** Rust
- **Deployment:** Native binary, runs as a **systemd service**

## Configuration
- **Model:** Single **centralized config file** (not one file per project).
- **Purpose of config:** For each registered project, define which script path to execute when a matching trigger event arrives, plus the trigger rule and secret for that project.
- **Trigger rules:** Flexible/configurable per project — not just "any push to any branch." Example: a custom rule like "run this script only on merge to `develop` branch."

## Webhook & Provider Support
- **MVP scope:** Multi-git-hosting support from day one — **not** GitLab-only for the initial release.
- Target providers: GitLab, GitHub, Codeberg/Gitea, and generically extensible to others over time.
- **Security:** HMAC secret validation per project, using each provider's signing mechanism (e.g. GitLab's `X-Gitlab-Token` / Secret Token header, GitHub's HMAC-SHA256 signature header, etc.). Each project in the config has its own secret.

## Execution Model
- **Queueing:** Incoming trigger executions are **queued, not parallel** — deploys run one at a time.
- **Locking:** Uses `.lock` file-based locking to prevent concurrent/overlapping runs (e.g. per-project or global lock, to be decided in design).
- **Script format:** Free-form — any executable can be run, not limited to `.sh` shell scripts.
- **Execution defaults:** By default, Blip runs the script fire-and-forget — no timeout enforcement, no output capture, no error tracking beyond basic execution. This tracking/observability behavior is **opt-in via config**, not the default.
  - When enabled: capture stdout/stderr, track exit code/success-failure, and (optionally) enforce an execution timeout.

## Logging & Observability
- Logs plus a **deploy history** (success/fail status, duration) queryable via CLI.
- On failure (when tracking is enabled): show the failure and the associated log/reason via CLI.

## Management Interface
- **Full CLI** for managing Blip — writes to and manages the centralized config file (e.g. adding projects, viewing status, viewing deploy history/logs, managing the running service).
- **REST API** — noted as a **future add-on feature**, not part of the initial phase.

## Notifications
- Integrations with services like Telegram, Discord, etc. for deploy failure/success alerts.
- **Not in the initial phase** — tracked as a future add-on feature.

## Phased Roadmap

### Phase 1 (MVP)
- Rust binary running as a systemd service.
- Centralized config file: per-project script path + trigger rule + secret.
- Multi-git-hosting webhook support (GitLab, GitHub, Codeberg/Gitea) with HMAC secret validation per project.
- Configurable trigger rules (event type + branch condition).
- Queued execution with `.lock`-based concurrency control.
- Fire-and-forget script execution by default.
- Opt-in per-project execution tracking: stdout/stderr capture, exit code, timeout enforcement.
- Deploy history (status, duration) and logs, queryable via full CLI.

### Phase 2 (Future / Add-on)
- REST API for managing Blip programmatically (in addition to CLI).
- Notification integrations (Telegram, Discord, etc.) on deploy failure/success.
- Possible web dashboard (not yet scoped/confirmed).

## Out of Scope (for now)
- Web dashboard/UI.
- Compute-minute billing or hosted-runner style execution.
- REST API (deferred to Phase 2).
- Notification integrations (deferred to Phase 2).
