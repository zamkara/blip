# Blip repository instructions

## Scope and precedence

This file applies to the entire repository. A more deeply nested `AGENTS.md`
may add narrower rules for its own directory, but it must not silently weaken
the architecture, security, or release rules defined here.

Follow an explicit user instruction when it conflicts with a workflow default.
Do not infer permission to commit, push, merge, install system files, modify a
running service, or delete user data. Read-only inspection and normal local
implementation work are allowed when they are necessary for the requested task.

## Product definition

Blip is a small native Rust service for self-hosted webhook-triggered
deployments. It authenticates a provider webhook, admits a delivery once,
places it in one global queue, and executes one administrator-controlled file.

Blip is not:

- a hosted runner;
- a workflow language;
- a replacement for GitLab event and branch filters;
- a deployment-script generator;
- a general CI/CD platform;
- a web dashboard in the initial release.

The configured executable owns checkout, build, deployment, application health
checks, and rollback. Blip owns authentication, delivery admission,
deduplication, queueing, serial execution, history, and service operations.

## Non-negotiable architecture

Preserve these decisions unless the user explicitly changes the product design:

1. The TOML project key is the project identity and URL segment. Do not add a
   duplicate `name` field.
2. Provider behavior uses nested provider-specific templates. Do not add a
   public generic `provider = "..."` switch.
3. GitLab is the first provider template. Future GitHub, Gitea, and Codeberg
   support must use their own nested schemas and signature contracts.
4. GitLab owns event and branch selection. Do not add `event`, `branch`, or
   `track` fields to Blip configuration.
5. `script` is an absolute path to one executable file. It is not a directory,
   shell expression, inline script, or list of build steps.
6. All projects share one bounded FIFO queue and one worker.
7. `blip.queue.lock` is the single global execution lock. Do not introduce
   per-project execution locks.
8. Runtime paths are derived internally from the history directory. Do not add
   project-level lock or delivery-registry path settings.
9. Webhook requests remain asynchronous: admission returns before deployment
   completion, while the worker waits for each script to preserve serial runs.
10. Journald remains the default stdout and stderr destination. History stores
    structured results, not unrestricted duplicate build logs.
11. Operational capabilities belong in the `blip` executable. Shell scripts
    should bootstrap the binary, then delegate configuration and service work
    to its CLI.

The intended minimal configuration is:

```toml
[projects.example-app]
script = "/srv/example-app/deploy"
gitlab.signing_token = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
```

Do not expand this schema merely to mirror controls already provided by GitLab.

## Current runtime contracts

The following behavior is implemented and must remain covered by tests:

- GitLab Standard Webhooks HMAC-SHA256 verification;
- optional legacy `X-Gitlab-Token` verification;
- signed-request timestamp tolerance;
- support for multiple space-separated signatures;
- keyed multi-project configuration;
- global queue capacity of 128 waiting jobs;
- serial script execution under one advisory execution lock;
- persistent delivery-ID claims using `webhook-id`;
- `Idempotency-Key` fallback for legacy GitLab deliveries;
- `202 queued` for a new admitted delivery;
- `202 duplicate` for an already accepted project and delivery ID;
- structured JSONL execution history;
- configuration, project, history, logs, queue, and systemd management through
  the CLI.

Runtime files derived from the history directory are:

- `blip-history.jsonl`: execution results;
- `blip-deliveries.jsonl`: accepted GitLab delivery IDs;
- `blip.queue.lock`: the global advisory execution lock.

The delivery registry is data, not another execution lock. It may be locked
briefly while an ID is checked and appended. A service restart can still lose a
waiting in-memory job; durable queue recovery remains separate roadmap work.

## Repository layout

Keep the repository root limited to primary project files and directories:

- `AGENTS.md`;
- `Cargo.toml` and `Cargo.lock`;
- `LICENSE`;
- `README.md`;
- `change.log`;
- `src/`;
- `docs/`;
- Git metadata and standard tool configuration when genuinely required.

Placement rules:

- Put configuration examples under `docs/examples/`.
- Put the single installation program at `docs/install.sh`.
- Put task-oriented documentation under `docs/wiki/`.
- Do not put sample, example, temporary, generated, log, runtime config, or
  testing artifacts in the repository root.
- Do not commit `target/`, local TOML configuration, JSONL runtime data, lock
  files, credentials, screenshots, or editor files.
- Keep `Cargo.lock` tracked because Blip is an executable application.

## Language and writing

All repository content must be written in professional English, including:

- source comments;
- CLI text and errors;
- README and wiki pages;
- examples;
- commit messages;
- `change.log` entries.

Use direct technical language. Do not pad documentation with slogans, repeated
explanations, agent-oriented instructions, or speculative claims. Describe the
behavior that exists, state limitations explicitly, and distinguish completed
work from roadmap work.

Use `example.com` hostnames and generic Unix paths in public documentation.
Never place a personal domain, username, home directory, device path, SSH key
path, webhook credential, or real token in tracked files.

## Documentation sources of truth

- `README.md` is the public overview and shortest successful path.
- `docs/wiki/Installation.md` owns installation, upgrade, service, and
  troubleshooting details.
- `docs/wiki/Configuration.md` owns the complete public TOML contract.
- `docs/wiki/GitLab.md` owns GitLab form values, authentication headers, and
  webhook testing behavior.
- `docs/wiki/Architecture.md` owns queue, locking, execution, history, and
  delivery-admission behavior.
- `docs/wiki/Security.md` owns threat boundaries and known security gaps.
- `docs/wiki/Roadmap.md` separates implemented behavior from remaining work.
- `change.log` is the canonical chronological record of repository changes.

When code behavior changes, update every affected source of truth in the same
work item. Do not leave completed features under roadmap TODO lists. Do not
describe planned behavior as implemented.

## Change log protocol

Append to `change.log` for every material work session before handoff.

Each entry must contain:

1. an RFC 3339 timestamp including seconds and the Asia/Jakarta offset, for
   example `2026-09-17T08:41:59+07:00`;
2. a concise scope;
3. the branch name when Git work is involved;
4. factual implementation notes;
5. an exact list of files added, modified, moved, or deleted;
6. validation commands and outcomes;
7. commit and push state when either matters.

Use the actual current time from the device. Do not invent or round timestamps.
Never write secrets, private hostnames, or personal paths into the log.

`docs/wiki/Development-log.md` contains historical public summaries. Do not use
it as a substitute for the detailed root `change.log` after this policy exists.

## Git workflow

### Before editing

1. Run `git status --short --branch`.
2. Preserve unrelated user changes.
3. Confirm the current branch and inspect relevant history.
4. Fetch only when current remote state matters and network access is allowed.
5. Start feature work from updated `dev` using a descriptive kebab-case branch,
   normally `feature/<scope>` or `fix/<scope>`.

Never perform feature implementation directly on `dev` when a feature branch is
available. Never push directly to `dev`; the user performs the merge so the
configured webhook can be tested.

### Commits

- Do not commit unless the user explicitly asks for a commit.
- When asked for one local commit, include all in-scope tracked and new files in
  exactly one commit relative to the requested base.
- Use a concise Conventional Commit subject describing the actual change.
- Do not amend, reset, squash, or rewrite user commits unless explicitly asked.
- Verify a clean worktree after committing.

### Synchronizing with dev

Before the first push of an unpublished feature branch:

1. fetch `origin/dev`;
2. rebase the local feature commit on the latest `origin/dev` when allowed;
3. verify `git rev-list --left-right --count origin/dev...HEAD` reports zero on
   the left;
4. rerun relevant tests after the rebase.

If the remote feature branch is protected or already shared, do not assume
force-push permission. Fetch both branches and choose a normal fast-forward-safe
integration, or report the protection constraint before rewriting history.
Never solve a protected-branch problem by pushing to `dev`.

### Push and merge

- Do not push unless the user explicitly asks.
- Push only the named feature branch.
- Prefer a normal push.
- Use `--force-with-lease` only when history rewrite was explicitly authorized
  and the remote branch permits it.
- Never use an unguarded `--force`.
- Never merge the feature branch into `dev`; leave that trigger opportunity to
  the user.
- After pushing, fetch and verify local/remote equality and the branch's
  ahead/behind count against `origin/dev`.

## Rust implementation standards

- Keep modules focused: configuration in `src/config.rs`, runtime behavior in
  `src/runtime.rs`, system integration in `src/system.rs`, and CLI routing in
  `src/main.rs` unless a new module has a clear responsibility.
- Use typed structures and explicit errors instead of loosely structured maps.
- Deny unknown public configuration fields so mistakes fail visibly.
- Preserve backward compatibility for stored history where practical using
  serde defaults for newly optional fields.
- Perform blocking filesystem locking and scans through `spawn_blocking` rather
  than blocking Tokio worker threads.
- Authenticate before delivery admission.
- Persist a delivery claim before queueing a new execution.
- Fail closed when authentication or persistent deduplication state cannot be
  trusted.
- Use constant-time comparisons for credentials and signatures.
- Never log credentials, raw signing tokens, or Secret tokens.
- Keep deployment payload text out of shell syntax and command arguments.
- Prefer deterministic derived runtime paths over additional configuration.
- Avoid dependencies when the standard library and existing crates provide a
  clear, safe implementation.

## Configuration rules

Configuration changes require special scrutiny because the public schema is a
product interface.

Before adding a field, answer all of these:

1. Is the value already controlled by the Git host?
2. Can it be derived safely from an existing global setting?
3. Is it runtime state rather than operator configuration?
4. Does adding it duplicate the project key or provider identity?
5. Can the binary expose the operation through a command instead?

If any answer indicates duplication, do not add the field.

Validation must cover project keys, absolute script paths, regular files,
executable mode, credentials, signing-token encoding, positive timestamp
tolerance, and required global destinations. Configuration output must redact
credentials by default. Revealing secrets must require an explicit option.

## Queue, locking, and delivery rules

- Maintain FIFO admission for new deliveries.
- Never allow two deployment scripts to run concurrently when they share the
  runtime directory.
- Keep one global execution lock, regardless of project count.
- Treat the existence of `blip.queue.lock` as normal; kernel lock ownership is
  the actual state.
- Scope deduplication by project key and delivery ID.
- Prefer `webhook-id`; accept `Idempotency-Key` only as the documented legacy
  fallback.
- Reject conflicting IDs when both headers are present.
- Return `202 duplicate` without adding another queue entry.
- Return `503` when registry integrity or availability prevents safe admission.
- Record the delivery ID in execution history for operator correlation.
- Keep deployment scripts idempotent as defense in depth.

Do not claim exactly-once external side effects. A crash around process
execution cannot make an arbitrary deployment script transactional. Describe
the implemented guarantee precisely as delivery admission deduplication.

## CLI and systemd rules

The binary is the operational interface. New operations should normally become
`blip` subcommands rather than undocumented shell commands.

Maintain support for:

- config path, show, validate, and set;
- project list, add/replace, and remove;
- filtered history;
- journald logs;
- queue and runtime-path inspection;
- systemd install, uninstall, status, start, stop, restart, enable, and disable;
- foreground serving with an explicit config path.

Read-only commands should run without root when permissions allow. System
configuration and systemd mutations may request `sudo` narrowly. The service
must not run as root.

## Installer rules

`docs/install.sh` is the only supported combined installer and setup program.
The README copy-and-paste command downloads this file from the `dev` branch.

The installer must:

- be valid Bash and work when invoked from Bash, Fish, or another shell through
  the documented Bash wrapper;
- run Git, Cargo, and rustup as the selected non-root build user;
- never assume root has the user's Rust toolchain or SSH identity;
- support HTTPS for public installation and explicit repository overrides;
- use an installer-owned source checkout;
- refuse dirty or mismatched existing source checkouts;
- fast-forward rather than silently rewrite source history;
- preserve valid configuration during upgrades;
- detect and back up the obsolete `[[projects]]` schema automatically;
- validate configuration through the installed Blip binary;
- delegate systemd unit creation and management to Blip;
- avoid deleting configuration, history, registry, source, or binaries during a
  normal service uninstall.

Any change to installer behavior must update README and Installation wiki text
and pass `bash -n docs/install.sh`.

## Security rules

- Prefer GitLab Signing tokens; retain Secret token support only for migration
  and compatibility.
- Never commit a real `whsec_...` value, Secret token, private key, certificate,
  tunnel credential, or access token.
- Examples must use unmistakably synthetic credentials.
- Keep configuration permissions restrictive.
- Preserve the service's non-root identity and `NoNewPrivileges=true` posture.
- Validate signed requests over the unmodified body.
- Enforce the timestamp replay window.
- Keep credential comparison constant-time.
- Do not expose private paths or credentials in errors, logs, history, docs, or
  `change.log`.
- Treat request-size limits, rate limiting, process-group control, sandboxing,
  and retention as unresolved until implementation and tests exist.

## Required validation

Run checks proportional to the change. For normal Rust behavior changes, the
minimum final gate is:

```bash
git diff --check
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release
bash -n docs/install.sh
```

Also perform focused tests for the changed behavior. Examples:

- authentication changes: valid, invalid, stale, altered-body, and multiple
  signature cases;
- queue changes: capacity, FIFO, concurrency, and closed-worker behavior;
- deduplication changes: first claim, concurrent claim, queued duplicate,
  completed duplicate, project scoping, malformed persistence, and restart;
- configuration changes: parse, validate, render, redact, save, and old-data
  compatibility;
- service changes: generated unit content and privilege behavior;
- installer changes: fresh install, upgrade, legacy config, and non-interactive
  environment behavior on a disposable host when available.

Do not report success when a command stopped before later checks. Distinguish a
test-environment limitation from a product failure, and record both accurately.

## Definition of done

A work item is complete only when:

1. implementation and focused tests agree;
2. formatting, tests, Clippy, release build, and relevant script checks pass;
3. public documentation describes the new behavior and limitations;
4. the roadmap moves completed work out of pending scope;
5. `change.log` contains a timestamped entry with exact files and validation;
6. no secrets, personal paths, private domains, or generated runtime data are
   present;
7. unrelated user changes remain intact;
8. Git status and commit/push state are reported honestly;
9. no commit, push, or merge has occurred without explicit authorization.

## Handoff format

Lead with the outcome. State:

- what changed;
- what remains intentionally unresolved;
- validation results;
- current branch;
- whether the worktree is clean;
- whether changes are uncommitted, committed locally, or pushed;
- whether `dev` was untouched.

Keep the handoff concise enough to act on, while linking directly to the most
important files when the interface supports local file links.
