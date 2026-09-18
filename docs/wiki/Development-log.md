# Development log

This page preserves early public development summaries. Starting on 2026-09-17, the root **change.log** is the canonical detailed work record. Neither log may contain credentials, private hostnames, or machine-specific paths.

## 2026-09-16T01:52:34+07:00

- Audited the repository structure, runtime implementation, public documentation, installer, Cargo metadata, and imported product requirements.
- Replaced the test-specific installer with one combined installation and initial-configuration script.
- Removed assumptions about a personal checkout, SSH key, webhook hostname, self-build branch, and desktop artifact directory.
- Reorganized documentation into task-focused wiki pages for installation, configuration, providers, architecture, security, and roadmap.
- Converted all imported requirements into implementation status, engineering tasks, test strategy, and acceptance criteria; no planning-source artifact is retained.
- Corrected package metadata to use the repository's Almatera Incubator License file.
- Preserved configuration examples under `docs/examples` and removed executable examples from the repository root.
- Recorded current implementation limits rather than presenting unfinished management, reliability, and security features as complete.

## 2026-09-16T02:12:58+07:00

- Replaced the project array with a keyed project map, removing duplicate project names.
- Replaced the generic provider field with a GitLab-specific nested template.
- Removed event and branch filters from Blip; these are configured in GitLab.
- Removed per-project tracking, timeout, and lock settings from the public configuration.
- Replaced per-project marker locks with one advisory lock shared by the global execution queue.
- Reduced the configuration example to one project header, one executable file path, and one GitLab token field.

## 2026-09-16T02:43:57+07:00

- Corrected the one-command installation path for devices that still contain the obsolete project-array configuration.
- Added automatic detection of the legacy **[[projects]]** schema, timestamped backup creation, and current-schema setup without requiring an extra installer flag.
- Kept valid existing configurations untouched and retained explicit **BLIP_RECONFIGURE=1** behavior for intentional replacement of other configurations.
- Updated the README and installation wiki to describe the exact upgrade behavior.

## 2026-09-17T08:37:53+07:00

- Implemented persistent GitLab delivery-ID deduplication with **webhook-id** and legacy **Idempotency-Key** support.
- Added an atomically claimed JSONL registry beside runtime history without adding project configuration or another execution lock.
- Added **202 duplicate**, malformed-ID handling, delivery-ID history correlation, and registry inspection through **blip queue**.
- Added tests for concurrent claims, queued duplicates, completed duplicates, project scoping, and persistence across service reconstruction.
- Updated the README, runtime architecture, GitLab guide, installation paths, security notes, and roadmap.

## 2026-09-18T20:34:29+07:00

- Replaced the process-local waiting queue with an append-only durable FIFO journal.
- Persisted queued, running, and completed delivery state with a stable sequence and the admitted script path.
- Added startup recovery for queued and interrupted running work while preserving the single global execution lock.
- Preserved old delivery-registry records as completed deduplication records.
- Extended queue inspection with queued, running, and completed counts.
- Made relative runtime paths deterministic so the installed service and CLI inspect the same files regardless of the shell's current directory.
- Added focused coverage for FIFO execution, exact capacity, concurrent and project-scoped admission, malformed persistence, legacy records, restart recovery, and recovery coordination with another active process.
- Updated the roadmap and operational documentation to describe durable waiting work and at-least-once interrupted-run recovery.

## 2026-09-18T21:17:32+07:00

- Added graceful SIGTERM and SIGINT handling that finishes the active deployment and leaves waiting entries durable for the next start.
- Added **blip --upgrade** and **blip -U** for fast-forward source updates, release builds, configuration validation, systemd unit updates, and service restart.
- Simplified first installation to a direct **curl | sudo bash** command with automatic non-root build-user detection.
- Fixed service installation to restart an already-running service after replacing its binary.
- Set the Phase 1 package version to **0.2.0** and documented that Phase 1 remains on **0.x** while Phase 2 begins at **1.0.0**.
