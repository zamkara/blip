#!/usr/bin/env bash
set -Eeuo pipefail

readonly BLIP_DEFAULT_REPOSITORY="https://gitlab.com/almateraincubator/utilities/blip.git"
readonly BLIP_DEFAULT_BRANCH="dev"
readonly BLIP_CONFIG_PATH="/etc/blip/blip.toml"
readonly BLIP_BINARY_PATH="/usr/local/bin/blip"

log() {
  printf '[blip] %s\n' "$*"
}

fail() {
  printf '[blip] error: %s\n' "$*" >&2
  exit 1
}

[[ "$(id -u)" -eq 0 ]] || fail "run this installer with sudo"

build_user="${BLIP_USER:-${SUDO_USER:-}}"
[[ -n "$build_user" && "$build_user" != "root" ]] || \
  fail "set BLIP_USER to the non-root account used to build Blip"
id "$build_user" >/dev/null 2>&1 || fail "build user does not exist: $build_user"

build_home="$(getent passwd "$build_user" | cut -d: -f6)"
build_group="$(id -gn "$build_user")"
[[ -n "$build_home" && -d "$build_home" ]] || fail "cannot determine home directory for $build_user"

repo_url="${BLIP_REPO_URL:-$BLIP_DEFAULT_REPOSITORY}"
repo_branch="${BLIP_REPO_BRANCH:-$BLIP_DEFAULT_BRANCH}"
source_dir="${BLIP_SOURCE_DIR:-/usr/local/src/blip}"
service_user="${BLIP_SERVICE_USER:-$build_user}"
service_group=""
build_path="$build_home/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

[[ "$source_dir" = /* ]] || fail "BLIP_SOURCE_DIR must be an absolute path"
case "$source_dir" in
  /|/etc|/home|/opt|/tmp|/usr|/usr/local|/var|/var/lib)
    fail "BLIP_SOURCE_DIR is too broad: $source_dir"
    ;;
esac
[[ "$service_user" != "root" ]] || fail "BLIP_SERVICE_USER must not be root"

run_as_builder() {
  runuser -u "$build_user" -- env \
    HOME="$build_home" \
    USER="$build_user" \
    LOGNAME="$build_user" \
    PATH="$build_path" \
    "$@"
}

install_build_dependencies() {
  if [[ "${BLIP_INSTALL_DEPENDENCIES:-1}" != "1" ]]; then
    fail "Git, curl, and a C compiler are required; install them or set BLIP_INSTALL_DEPENDENCIES=1"
  fi

  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y build-essential ca-certificates curl git
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y ca-certificates curl gcc git
  elif command -v zypper >/dev/null 2>&1; then
    zypper --non-interactive install -y ca-certificates curl gcc git
  elif command -v pacman >/dev/null 2>&1; then
    pacman --sync --refresh --needed --noconfirm base-devel ca-certificates curl git
  else
    fail "unsupported package manager; install Git, curl, and a C compiler manually"
  fi
}

missing_build_tool=0
for command_name in git curl cc; do
  command -v "$command_name" >/dev/null 2>&1 || missing_build_tool=1
done
if [[ "$missing_build_tool" -eq 1 ]]; then
  log "installing native build dependencies"
  install_build_dependencies
fi

for command_name in getent grep install runuser systemctl; do
  command -v "$command_name" >/dev/null 2>&1 || fail "required command is missing: $command_name"
done

rustup_installer=""
cleanup() {
  [[ -z "$rustup_installer" || ! -f "$rustup_installer" ]] || rm -f -- "$rustup_installer"
}
trap cleanup EXIT

if ! run_as_builder bash -c 'command -v cargo >/dev/null 2>&1'; then
  log "installing the stable Rust toolchain for $build_user"
  rustup_installer="$(mktemp)"
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$rustup_installer"
  chown "$build_user:$build_group" "$rustup_installer"
  chmod 0700 "$rustup_installer"
  run_as_builder "$rustup_installer" -y --profile minimal --default-toolchain stable
fi

if run_as_builder bash -c 'command -v rustup >/dev/null 2>&1'; then
  log "selecting the stable Rust toolchain for $build_user"
  run_as_builder rustup toolchain install stable --profile minimal
  run_as_builder rustup default stable
  cargo_command=(cargo +stable)
else
  cargo_command=(cargo)
fi

source_parent="$(dirname "$source_dir")"
[[ -d "$source_parent" ]] || install -d -m 0755 "$source_parent"
[[ ! -L "$source_dir" ]] || fail "source path must not be a symbolic link: $source_dir"

if [[ -e "$source_dir" && ! -d "$source_dir/.git" ]]; then
  if [[ ! -d "$source_dir" || -n "$(find "$source_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    fail "source path exists but is not an empty directory or Git checkout: $source_dir"
  fi
fi

if [[ ! -d "$source_dir/.git" ]]; then
  log "cloning Blip into $source_dir"
  install -d -m 0755 -o "$build_user" -g "$build_group" "$source_dir"
  run_as_builder git clone --branch "$repo_branch" --single-branch "$repo_url" "$source_dir"
else
  current_origin="$(run_as_builder git -C "$source_dir" remote get-url origin)"
  [[ "$current_origin" == "$repo_url" ]] || \
    fail "source checkout origin is $current_origin, expected $repo_url"
  [[ -z "$(run_as_builder git -C "$source_dir" status --porcelain)" ]] || \
    fail "source checkout contains local changes: $source_dir"
  log "updating the existing source checkout"
  run_as_builder git -C "$source_dir" fetch origin "$repo_branch"
  if run_as_builder git -C "$source_dir" show-ref --verify --quiet "refs/heads/$repo_branch"; then
    run_as_builder git -C "$source_dir" switch "$repo_branch"
  else
    run_as_builder git -C "$source_dir" switch --create "$repo_branch" --track "origin/$repo_branch"
  fi
  run_as_builder git -C "$source_dir" merge --ff-only "origin/$repo_branch"
fi

log "building the release binary"
run_as_builder "${cargo_command[@]}" build --locked --release --manifest-path "$source_dir/Cargo.toml"
install -m 0755 "$source_dir/target/release/blip" "$BLIP_BINARY_PATH"

if ! id "$service_user" >/dev/null 2>&1; then
  command -v useradd >/dev/null 2>&1 || fail "cannot create service user because useradd is unavailable"
  log "creating service account: $service_user"
  useradd --system --create-home --home-dir /var/lib/blip --shell /usr/sbin/nologin "$service_user"
fi
service_group="$(id -gn "$service_user")"

install -d -m 0750 -o root -g "$service_group" /etc/blip
install -d -m 0750 -o "$service_user" -g "$service_group" /var/lib/blip

prompt_value() {
  local variable_name="$1"
  local label="$2"
  local default_value="${3:-}"
  local current_value="${!variable_name:-}"

  [[ -n "$current_value" ]] && return
  if [[ ! -r /dev/tty ]]; then
    [[ -n "$default_value" ]] || fail "$variable_name is required in non-interactive mode"
    printf -v "$variable_name" '%s' "$default_value"
    return
  fi
  if [[ -n "$default_value" ]]; then
    read -r -p "$label [$default_value]: " current_value </dev/tty
    current_value="${current_value:-$default_value}"
  else
    read -r -p "$label: " current_value </dev/tty
  fi
  printf -v "$variable_name" '%s' "$current_value"
}

configure_required=0
if [[ ! -f "$BLIP_CONFIG_PATH" ]]; then
  configure_required=1
elif [[ "${BLIP_RECONFIGURE:-0}" == "1" ]]; then
  log "configuration replacement requested"
  configure_required=1
elif grep -Eq '^[[:space:]]*\[\[projects\]\][[:space:]]*(#.*)?$' "$BLIP_CONFIG_PATH"; then
  log "legacy project-array configuration detected; it will be backed up and replaced"
  configure_required=1
else
  log "preserving existing configuration: $BLIP_CONFIG_PATH"
fi

if [[ "$configure_required" -eq 1 ]]; then
  project_key="${BLIP_PROJECT_KEY:-}"
  deploy_script="${BLIP_SCRIPT:-}"
  signing_token="${BLIP_SIGNING_TOKEN:-}"
  secret_token="${BLIP_SECRET_TOKEN:-${BLIP_SECRET:-}}"

  prompt_value project_key "Project key used in the webhook URL"
  prompt_value deploy_script "Absolute path to the deployment executable"

  [[ "$project_key" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]] || \
    fail "project key may contain only letters, digits, dots, underscores, and hyphens"
  [[ "$deploy_script" = /* ]] || fail "deployment executable must use an absolute path"
  [[ -x "$deploy_script" ]] || fail "deployment executable is missing or not executable: $deploy_script"
  [[ -f "$deploy_script" ]] || fail "deployment executable must be a file: $deploy_script"
  runuser -u "$service_user" -- test -x "$deploy_script" || \
    fail "$service_user cannot execute $deploy_script"

  if [[ -f "$BLIP_CONFIG_PATH" ]]; then
    backup_path="$BLIP_CONFIG_PATH.$(date -u +%Y%m%dT%H%M%SZ).bak"
    cp --preserve=mode,ownership "$BLIP_CONFIG_PATH" "$backup_path"
    log "saved the previous configuration to $backup_path"
    rm -f -- "$BLIP_CONFIG_PATH"
  fi

  project_command=(
    "$BLIP_BINARY_PATH"
    --config "$BLIP_CONFIG_PATH"
    project add
    --key "$project_key"
    --script "$deploy_script"
  )
  if [[ -n "$signing_token" ]]; then
    project_command+=(--signing-token "$signing_token")
  elif [[ -n "$secret_token" ]]; then
    project_command+=(--secret-token "$secret_token")
  fi
  "${project_command[@]}"
fi

log "validating configuration"
if ! "$BLIP_BINARY_PATH" --config "$BLIP_CONFIG_PATH" config validate; then
  fail "configuration is invalid; fix it or rerun with BLIP_RECONFIGURE=1 to replace it"
fi

"$BLIP_BINARY_PATH" --config "$BLIP_CONFIG_PATH" service install --user "$service_user"

log "installation complete"
log "configuration: $BLIP_CONFIG_PATH"
log "service status: blip service status"
log "webhook path: /webhook/<project-key>"
