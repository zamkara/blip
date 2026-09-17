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
