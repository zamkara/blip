# Blip wiki

This wiki describes the Phase 1 release, version **0.3.0**, and the work still required. Runtime credentials and machine-specific files do not belong in this repository. Phase 1 remains on **0.x** versions; Phase 2 begins with **1.0.0**.

| Page | Purpose |
| --- | --- |
| [Installation](Installation.md) | Install, configure, upgrade with **blip -U**, inspect, and remove the systemd service. |
| [Configuration](Configuration.md) | Minimal TOML schema, defaults, multiple projects, and migration from the old schema. |
| [GitLab](GitLab.md) | Exact webhook form values, Signing token behavior, Secret token fallback, and testing. |
| [GitHub](GitHub.md) | GitHub secret, signature header, delivery ID, and setup limits. |
| [Gitea](Gitea.md) | Native Gitea signature and delivery headers. |
| [Codeberg](Codeberg.md) | Native Forgejo signature and delivery headers used by Codeberg. |
| [Architecture](Architecture.md) | Queue ownership, the single global lock, script execution, and history. |
| [Security](Security.md) | Credentials, endpoint exposure, service permissions, and deployment-script safety. |
| [Roadmap](Roadmap.md) | Product direction, reliability, observability, CLI work, tests, and acceptance criteria. |
| [Development log](Development-log.md) | Timestamped records of repository work. |

## Repository layout

~~~text
.
├── AGENTS.md
├── Cargo.toml
├── LICENSE
├── README.md
├── change.log
├── docs
│   ├── examples
│   │   └── blip.toml.example
│   ├── install.sh
│   └── wiki
│       └── *.md
└── src
    └── main.rs
~~~

The root contains project metadata, repository instructions, the canonical change log, the license, the primary README, and Rust source. Examples remain under **docs/examples**. The single installer remains under **docs**. Wiki pages remain under **docs/wiki**.

## Responsibility boundary

- The Git host owns event selection, retries, and delivery configuration. GitLab also provides the branch filter used by its template.
- Blip owns authentication, durable queue admission, serial execution, and basic history.
- The configured executable file owns checkout, build, deployment, health checks, and rollback.

This boundary avoids reproducing each Git host's webhook form inside Blip configuration.
