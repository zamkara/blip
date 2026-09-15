#!/usr/bin/env bash
set -eu

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
log_file="$repo_root/.blip-test-deploy.log"
printf '%s deploy received in %s\n' "$(date --iso-8601=seconds)" "$repo_root" >> "$log_file"
printf 'Blip test deploy executed: %s\n' "$(date --iso-8601=seconds)"
