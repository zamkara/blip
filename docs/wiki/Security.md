# Security

Blip converts an Internet request into local process execution. The GitLab settings, Blip configuration, service identity, and executable file form one trust boundary.

## Authentication

- Prefer a GitLab Signing token.
- Use a different token for every project.
- Keep **/etc/blip/blip.toml** readable only by root and the service group.
- Never store real credentials in the repository, shell history, screenshots, logs, or issue reports.
- Keep the host clock synchronized because signed requests have a replay window.
- Rotate any token that has been exposed.

The legacy Secret token authenticates a shared header. A Signing token additionally verifies the raw request body and timestamp.

## GitLab trigger controls

Configure only events that should execute a deployment and use GitLab's branch filter. This is the authorization boundary for event type and branch because Blip intentionally does not duplicate those rules.

Anyone able to edit the GitLab webhook can broaden its triggers. Protect Maintainer and Owner access accordingly.

## Network

- Keep the default loopback bind when a tunnel or reverse proxy runs locally.
- Publish only the required HTTPS hostname.
- Keep SSL verification enabled in GitLab.
- Add request-size and rate limits at the proxy; Blip does not yet enforce either.
- Do not expose the listener directly on a public interface unless the host firewall and TLS termination are deliberately configured.

## Service account

Do not run Blip as root. The account needs:

- read and execute access through every component of the script path;
- write access to **/var/lib/blip**;
- only the application permissions required by deployment scripts.

The installer uses **NoNewPrivileges=true** and a restrictive umask. Stronger systemd filesystem restrictions must be tailored to the actual deployment paths.

## Deployment script

- Make it an absolute, administrator-controlled executable file.
- Set its working directory explicitly.
- Validate the repository remote, branch, and expected revision before deployment.
- Keep repeated runs safe. Blip deduplicates accepted GitLab delivery IDs, but an execution interrupted before completion is durably recorded may run again after restart.
- Handle build failure, health checking, and rollback inside the script.
- Avoid printing credentials; stdout and stderr are stored in journald.
- Do not allow untrusted users to modify the file.

Webhook payload data is not passed as a command argument, which prevents payload text from becoming shell syntax.

## Queue lock

One advisory lock coordinates the global queue. Do not delete **blip.queue.lock** merely because it exists; the persistent file is normal. Use process inspection and service logs to diagnose a genuinely blocked execution.

## Known gaps

- No request body limit or rate limiter.
- No exactly-once guarantee for external script side effects; interrupted running entries use at-least-once recovery.
- No execution timeout, cancellation, or process-group control.
- No per-project Unix identity or process sandbox.
- No history retention or structured log redaction.
- No graceful queue drain during shutdown.

These gaps are listed in the [roadmap](Roadmap.md) and prevent claiming production hardening.
