# Kitchen Branch

This branch (`kitchen`) is dedicated solely to running GitHub Actions CI/CD workflows for `blip`.
It does not contain project source code, keeping the `dev` and other operational branches clean of `.github/workflows`.

## How it works

1. Whenever code is pushed / mirrored to the `dev` branch, GitHub Actions triggers the release & build workflow.
2. The workflow checks out the `dev` branch to build multi-architecture binaries (Linux x86_64, aarch64, armv7, riscv64, etc.).
3. A GitHub Release is dynamically generated using the commit hash and detailed commit changelogs.
