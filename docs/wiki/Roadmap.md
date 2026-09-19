# Product direction and roadmap

## Purpose

Blip replaces manual SSH, pull, and deploy work for operators running many applications on one server. Hosted services such as GitLab CI/CD, GitHub Actions, and Blacksmith can be excessive for a webhook-triggered local deployment; GitLab's 400-minute monthly free-tier allowance was the original concrete constraint.

The product remains a native Rust systemd service with one centralized configuration and a full management CLI. It is not a hosted runner, workflow language, billing platform, or Jenkins replacement.

Phase 1 uses pre-stable **0.x** releases while provider and operational contracts are completed. Phase 2 begins with **1.0.0** and a stable public CLI and configuration contract.

## Design decisions

The current architecture follows these decisions:

- The project TOML key is the endpoint identity; no duplicate name.
- Provider behavior is represented by a nested provider-specific template; no generic provider field.
- GitLab, GitHub, Gitea, and Codeberg have separate implemented templates.
- Git hosts own event controls and any available branch filtering; Blip does not duplicate those rules.
- A project points to one executable file.
- All accepted requests share one FIFO queue.
- Queue state is durable across service restarts.
- One advisory lock coordinates queue execution across Blip processes.
- The HTTP request is asynchronous, but the worker waits for each script to preserve serial execution.
- Basic result history is always recorded; stdout and stderr remain in service logs.

## Current implementation

- GitLab Signing token verification using Standard Webhooks.
- Optional legacy GitLab Secret token compatibility.
- GitHub **X-Hub-Signature-256** authentication and **X-GitHub-Delivery** deduplication.
- Native Gitea **X-Gitea-Signature** authentication and **X-Gitea-Delivery** deduplication.
- Native Codeberg/Forgejo **X-Forgejo-Signature** authentication and **X-Forgejo-Delivery** deduplication.
- Timestamp replay window for signed requests.
- Keyed multi-project TOML configuration.
- Absolute executable-file validation.
- Durable 128-entry waiting queue and one worker.
- One global advisory lock derived from the runtime directory.
- Persistent GitLab delivery-ID deduplication using **webhook-id** with legacy **Idempotency-Key** fallback.
- Recovery of queued and interrupted running deliveries after restart.
- Graceful SIGTERM and SIGINT handling that finishes the active job and preserves waiting entries.
- Success/failure, duration, exit code, and spawn-error history.
- CLI commands for serving, global configuration, project CRUD, filtered history, logs, queue inspection, and systemd management.
- In-place upgrade through **blip --upgrade** and **blip -U**, including service restart.
- One combined installation and setup script for systemd.

## Phase 1 — Complete the initial release

### Queue reliability

- Record queue rejection and lock wait duration.
- Add execution timeout and cancellation without allowing the next job to overlap a surviving process.
- Define retry policy without turning Blip into a workflow engine.

### Execution and observability

- Add opt-in detailed output capture without changing the minimal project schema.
- Limit and redact captured output.
- Add history rotation and retention.
- Record signal exits and process-group failures.
- Add health, readiness, and metrics endpoints.
- Keep journald as the default stdout/stderr destination.

### Management CLI completion

- Add a dedicated project-edit command; replacement is currently handled by project add with **--replace**.
- Add enable/disable state per project.
- Rotate credentials safely.
- Add history filtering by time; project, result, and limit filters already exist.
- Add service health details beyond systemd status.

### Validation and security

- Verify runtime read/execute permission as the configured service identity.
- Validate bind and history destinations before startup.
- Add request-size and rate limits.
- Add per-project service identities or process sandboxing.
- Add structured audit events without logging credentials.
- Test signed-request edge cases, including multiple signatures and clock skew.

## Phase 2 — Add-ons

- Versioned REST management API in addition to the CLI.
- Telegram, Discord, and other deployment notifications.
- Configurable retry policy if operational evidence justifies it.
- Optional web dashboard only after API authentication and authorization are stable.

## Test plan

### Unit

- Keyed TOML parsing and invalid project keys.
- Script path, file type, and executable-mode validation.
- Signing and Secret token verification.
- Stale timestamp and altered-body rejection.
- Global lock path and advisory lock behavior.
- Persistent and concurrent delivery-ID claim behavior.
- Durable FIFO order, exact waiting capacity, and interrupted-run recovery.
- History serialization and project filtering.

### Integration

- HTTP 202, 401, 404, and queue-full 503 behavior.
- FIFO execution across different projects.
- Two Blip processes sharing one runtime lock.
- Success, non-zero exit, and spawn failure.
- Restart recovery through the systemd service with queued and interrupted work.
- Graceful shutdown with idle, active, waiting-lock, and queued work.
- Upgrade from the previous release with configuration and history preserved.

### End to end

1. Install from published documentation on a disposable systemd host.
2. Route an example HTTPS hostname to loopback Blip.
3. Configure the provider webhook, required events, and any available branch filter.
4. Merge into the target branch.
5. Verify one history record, one script run, the expected revision, and an identifiable artifact outside the source checkout.
6. Repeat with an invalid credential, a failing script, duplicate delivery, and service restart for each provider.

## Acceptance criteria

The initial release is complete when:

1. Each implemented provider template passes official fixture-based signature tests.
2. Blip configuration contains no duplicated event or branch policy.
3. Every authenticated request is either queued once or rejected with a documented status.
4. No two scripts run concurrently, including across two Blip processes sharing the runtime directory.
5. Every completed or failed start produces an accurate history record.
6. The CLI safely manages keyed projects and redacts credentials.
7. A fresh operator can install, configure, test, upgrade, diagnose, and remove Blip using the wiki alone.
8. An end-to-end merge creates exactly one verifiable artifact from the intended revision.

## Deferred scope

- Hosted compute and compute-minute billing.
- General workflow orchestration.
- Web dashboard in the initial phase.
- REST API and notifications in the initial phase.
