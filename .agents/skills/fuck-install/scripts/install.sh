#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2024-2026 Temps Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# Installer for a "fuck" server (the Temps platform under the fork's name):
# Docker, a TimescaleDB container, the binary, setup and a systemd unit.
# ClickHouse stays off (PG-only) so it fits 1-2 vCPU / 2-4 GB. Idempotent: a
# re-run never rotates a secret and never asks again.
#
#   sudo install.sh                    # asks environment, email, password, names, domain
#   sudo install.sh --env development  # builds this checkout in Docker, loopback, debug logs
#   sudo install.sh --mode local       # production binary, loopback only
#   sudo install.sh --check            # read-only preflight
#   sudo install.sh --upgrade          # DB backup, new binary (or rebuild), automatic rollback
#   sudo install.sh --uninstall        # removes service and binary, keeps data (--purge deletes it)
#   unattended: --env --email --first-name --last-name --password-file [--domain]
#               [--version vX.Y.Z | --channel stable|beta|nightly]   (production)
#
# Without a domain, Enter takes <public-ip>.sslip.io (a .io wildcard DNS name).
# Run it from a repository checkout: production reuses scripts/install.sh for
# the verified release, development builds the checkout with its Dockerfile.
set -euo pipefail
export LC_ALL=C # deterministic character classes for the input validation below

# The name operators see: paths, system user, service alias, CLI command.
BRAND=fuck

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../../../.." && pwd)
BINARY_INSTALLER=$REPO_ROOT/scripts/install.sh
UNIT_TEMPLATE=$SCRIPT_DIR/../templates/temps.service

ETC=/etc/$BRAND
CONF=$ETC/install.conf
MARKER=$ETC/.managed-by-installer
SETUP_DONE=$ETC/.setup-done
PREFIX=/opt/$BRAND
DATA=/var/lib/$BRAND
# The binary looks for exactly this path (SYSTEMD_UNIT_PATH in
# crates/temps-cli/src/service_definition.rs); the unit adds an $BRAND alias.
UNIT=/etc/systemd/system/temps.service
LOG=/var/log/$BRAND-install.log
DB_CONTAINER=$BRAND-postgres
DB_VOLUME=$BRAND-pgdata
DB_PORT=16432
# shortcut: a major-version tag, not a digest; pin by digest once the fork publishes its own DB image.
DB_IMAGE=timescale/timescaledb-ha:pg18
CONSOLE_ADDR=127.0.0.1:8081

env="" mode="" mode_flag="" domain_flag="" email="" first_name="" last_name="" password=""
password_file="" version="" channel="" check_only=false upgrade=false uninstall=false purge=false
fails=0

usage() { sed -n '5,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
step() { printf '==> %s\n' "$*"; }
ok() { printf '  ok    %s\n' "$*"; }
warn() { printf '  warn  %s\n' "$*"; }
fail() { printf '  FAIL  %s\n' "$*"; fails=$((fails + 1)); }
need() { [[ $# -ge 2 && -n $2 && $2 != -* ]] || die "$1 requires a value"; }
managed() { [[ -f $MARKER ]]; }
port_free() { ! ss -Htln "sport = :$1" | grep -q .; }

while (($#)); do
  case $1 in
    --email) need "$@"; email=$2; shift 2 ;;
    --first-name) need "$@"; first_name=$2; shift 2 ;;
    --last-name) need "$@"; last_name=$2; shift 2 ;;
    --password-file) need "$@"; password_file=$2; shift 2 ;;
    --domain) need "$@"; domain_flag=$2; shift 2 ;;
    --env) need "$@"; env=$2; shift 2 ;;
    --mode) need "$@"; mode_flag=$2; shift 2 ;;
    --version) need "$@"; version=$2; shift 2 ;;
    --channel) need "$@"; channel=$2; shift 2 ;;
    --check) check_only=true; shift ;;
    --upgrade) upgrade=true; shift ;;
    --uninstall) uninstall=true; shift ;;
    --purge) purge=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

valid_email() { [[ $1 =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,63}$ ]]; }
valid_domain() { [[ $1 =~ ^([A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}$ ]]; }
valid_name() { [[ -n $1 && ${#1} -le 50 && $1 != -* && ! $1 =~ [[:cntrl:]] ]]; }
valid_env() { [[ $1 == production || $1 == development ]]; }
# Mirrors validate_password_complexity in temps-auth: 8-128 chars, upper, lower, digit, symbol.
valid_password() {
  ((${#1} >= 8 && ${#1} <= 128)) &&
    [[ $1 =~ [[:upper:]] && $1 =~ [[:lower:]] && $1 =~ [[:digit:]] && $1 =~ [^[:alnum:]] ]]
}

# Choices from the first run are the defaults, so a bare re-run never flips a
# loopback install to public. Parsed, never sourced.
saved() { [[ -r $CONF ]] && sed -n "s/^$1=//p" "$CONF" | head -1 || true; }
saved_domain=$(saved domain)
saved_env=$(saved env)
env=${env:-$saved_env}
mode=${mode_flag:-$(saved mode)}
domain=${domain_flag:-$saved_domain}
email=${email:-$(saved email)}

# Trust boundary: every flag value ends up in a command line or a unit file.
[[ -z $env ]] || valid_env "$env" || die "--env must be production or development"
[[ -z $mode_flag || $mode_flag == public || $mode_flag == local ]] || die "--mode must be public or local"
[[ -z $email ]] || valid_email "$email" || die "invalid --email"
[[ -z $first_name ]] || valid_name "$first_name" || die "invalid --first-name (1-50 characters)"
[[ -z $last_name ]] || valid_name "$last_name" || die "invalid --last-name (1-50 characters)"
[[ -z $domain_flag ]] || valid_domain "$domain_flag" || die "invalid --domain"
[[ -z $version || $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.]+)?$ ]] || die "invalid --version (expected vX.Y.Z)"
[[ -z $channel || $channel =~ ^(stable|beta|nightly)$ ]] || die "--channel must be stable, beta or nightly"
[[ -z $version || -z $channel ]] || die "pass --version or --channel, not both"
$uninstall || ! $purge || die "--purge only goes with --uninstall"
! { $upgrade && $uninstall; } || die "pass --upgrade or --uninstall, not both"
if [[ -x $PREFIX/bin/temps && (-n $version || -n $channel) ]] && ! $upgrade; then
  die "this host already runs $BRAND: change its version with --upgrade"
fi
if [[ -n $saved_domain && -n $domain_flag && $domain_flag != "$saved_domain" ]]; then
  die "domain was set to $saved_domain at install time; change it in the console (Settings > Domains)"
fi

preflight() {
  step "Preflight (read-only)"
  [[ $(uname -s) == Linux ]] && ok "Linux" || fail "Linux is required (got $(uname -s))"
  [[ $EUID -eq 0 ]] && ok "root" || fail "run as root (sudo)"
  [[ -d /run/systemd/system ]] && ok "systemd" || fail "systemd is required to supervise $BRAND"
  case $(uname -m) in
    x86_64 | aarch64 | arm64) ok "arch $(uname -m)" ;;
    *) fail "unsupported architecture $(uname -m) (amd64/arm64 only)" ;;
  esac
  local glibc
  glibc=$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || true)
  if [[ -n $glibc ]] && printf '2.35\n%s\n' "$glibc" | sort -V -C; then
    ok "glibc $glibc"
  else
    fail "glibc >= 2.35 is required by the release binary (found ${glibc:-none})"
  fi

  local mem_mb free_gb
  mem_mb=$(awk '/^MemTotal/ {print int($2 / 1024)}' /proc/meminfo)
  if ((mem_mb < 1500)); then fail "${mem_mb} MB RAM; the platform + TimescaleDB need about 2 GB"
  elif ((mem_mb < 3800)); then warn "${mem_mb} MB RAM: small server, ClickHouse stays off"
  else ok "${mem_mb} MB RAM"; fi
  free_gb=$(df -Pk /var/lib | awk 'NR == 2 {print int($4 / 1048576)}')
  if ((free_gb < 10)); then fail "${free_gb} GB free under /var/lib; need 10+ (the DB image is ~3 GB)"
  elif ((free_gb < 20)); then warn "${free_gb} GB free under /var/lib; 20+ recommended"
  else ok "${free_gb} GB free under /var/lib"; fi

  local c missing=()
  for c in curl openssl ss awk sed; do command -v "$c" >/dev/null || missing+=("$c"); done
  ((${#missing[@]} == 0)) && ok "tools present" || fail "missing commands: ${missing[*]}"

  if [[ $env == development ]]; then
    if ((mem_mb >= 7500)); then ok "enough RAM to build the checkout"
    else warn "${mem_mb} MB RAM: building the checkout wants about 8 GB and may run out of memory"; fi
    [[ -f $REPO_ROOT/Dockerfile ]] && ok "Dockerfile found for the development build" ||
      fail "no Dockerfile in $REPO_ROOT: development builds need the full checkout"
  fi

  if command -v docker >/dev/null; then
    ok "docker $(docker --version 2>/dev/null | awk '{print $3}' | tr -d ,)"
    docker compose version >/dev/null 2>&1 ||
      warn "docker compose plugin missing: Compose deployments will not work until it is installed"
    if [[ $env == development ]] && ! docker buildx version >/dev/null 2>&1; then
      command -v apt-get >/dev/null && warn "docker buildx missing: the distro package docker-buildx will be installed" ||
        fail "docker buildx is missing and there is no apt-get to install it"
    fi
  elif command -v apt-get >/dev/null; then
    warn "docker not found: the distro packages docker.io and docker-compose-v2 will be installed"
  else
    fail "docker not found and no apt-get: install Docker Engine first (https://docs.docker.com/engine/install/)"
  fi

  if [[ -e $UNIT ]] && ! managed; then
    fail "$UNIT exists but was not created by this installer (a deploy.sh install?); refusing to touch it"
  fi
  if command -v docker >/dev/null && docker inspect "$DB_CONTAINER" >/dev/null 2>&1 && ! managed; then
    fail "container $DB_CONTAINER exists but was not created by this installer"
  fi

  if managed && systemctl is-active --quiet temps; then
    ok "ports held by the managed $BRAND service (re-run)"
  else
    local p owner
    for p in "${HTTP_ADDR##*:}" "${TLS_ADDR##*:}" "${CONSOLE_ADDR##*:}"; do
      if port_free "$p"; then ok "port $p free"; continue; fi
      owner=$(ss -Htlnp "sport = :$p" | grep -oE 'users:\(\("[^"]+"' | head -1 | cut -d'"' -f2)
      fail "port $p is in use by ${owner:-another process}"
    done
  fi
  if ! { command -v docker >/dev/null && docker inspect "$DB_CONTAINER" >/dev/null 2>&1; }; then
    port_free "$DB_PORT" && ok "port $DB_PORT free (database, loopback)" || fail "port $DB_PORT (database) is in use"
  fi
  [[ -f $BINARY_INSTALLER && -f $UNIT_TEMPLATE ]] && ok "running from a repository checkout" ||
    fail "scripts/install.sh or the unit template is missing: run this from a repository checkout"

  ((fails == 0)) || die "preflight found $fails problem(s); nothing was changed"
}

# ask VAR LABEL VALIDATOR [DEFAULT]: keeps a valid flag value, otherwise asks on
# the terminal until the answer is valid; without a terminal it stops.
ask() {
  local value=${!1} reply
  until "$3" "$value"; do
    [[ -z $value ]] || printf '  not valid: %s\n' "$value" >&2
    [[ -t 0 ]] || die "$2 is missing or invalid: pass it as a flag when not running in a terminal"
    read -rp "$2${4:+ [$4]}: " reply
    value=${reply:-${4:-}}
  done
  printf -v "$1" '%s' "$value"
}

ask_password() {
  if [[ -n $password_file ]]; then
    [[ -r $password_file ]] || die "cannot read --password-file $password_file"
    IFS= read -r password <"$password_file" || true
    valid_password "$password" ||
      die "the password in $password_file needs 8-128 characters with an uppercase letter, a lowercase letter, a digit and a symbol"
    return
  fi
  [[ -t 0 ]] || die "admin password is missing: pass --password-file when not running in a terminal"
  local again
  while :; do
    IFS= read -rsp "Admin password: " password
    echo
    if ! valid_password "$password"; then
      echo "  needs 8-128 characters with an uppercase letter, a lowercase letter, a digit and a symbol" >&2
      continue
    fi
    IFS= read -rsp "Repeat the password: " again
    echo
    [[ $password == "$again" ]] && break
    echo "  the two entries differ" >&2
  done
}

ask_domain() {
  if [[ $mode == local ]]; then domain=127-0-0-1.sslip.io; return; fi
  local ip fallback=""
  if [[ -z $domain ]]; then
    # sslip.io is a wildcard DNS: <a-b-c-d>.sslip.io and every subdomain resolve to that IP.
    ip=$(curl -4 -fsS --max-time 5 https://api.ipify.org 2>/dev/null ||
      curl -4 -fsS --max-time 5 https://ifconfig.me 2>/dev/null || true)
    [[ $ip =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] && fallback=${ip//./-}.sslip.io
    [[ -t 0 ]] || domain=$fallback
  fi
  ask domain "Domain (Enter keeps the .io fallback)" valid_domain "$fallback"
}

# The environment comes first: it decides the binary source, the network mode
# and the log level. Development runs this checkout's code, on loopback only.
choose_env() {
  if [[ -z $env && ! -f $SETUP_DONE && -t 0 ]] && ! $check_only; then
    ask env "Environment (production or development)" valid_env production
  fi
  env=${env:-production}
  [[ -z $saved_env || $env == "$saved_env" ]] ||
    die "this host was installed for $saved_env: uninstall first to switch it to $env"
  if [[ $env == development ]]; then
    [[ $mode_flag != public ]] || die "development installs stay on loopback: drop --mode public"
    [[ -z $domain_flag ]] || die "development installs use 127-0-0-1.sslip.io: drop --domain"
    [[ -z $version && -z $channel ]] || die "development builds this checkout: --version and --channel are for production"
    mode=local
  fi
  mode=${mode:-public}
  [[ -z $domain_flag || $mode == public ]] || die "--domain needs --mode public"
  local bind=0.0.0.0
  DISABLE_HTTPS_REDIRECT=false
  if [[ $mode == local ]]; then
    bind=127.0.0.1
    DISABLE_HTTPS_REDIRECT=true
  fi
  HTTP_ADDR=$bind:80
  TLS_ADDR=$bind:443
  LOG_LEVEL=info
  [[ $env == production ]] || LOG_LEVEL=debug
}

gather_inputs() {
  if [[ -f $SETUP_DONE ]]; then return; fi
  step "Administrator and domain"
  ask email "Admin email" valid_email
  ask first_name "First name" valid_name
  ask last_name "Last name" valid_name
  ask_password
  ask_domain
}

# Writes stdin to $1 with mode $2 only when the content differs; returns 0 if it changed.
write_if_changed() {
  local tmp
  tmp=$(mktemp "$1.XXXXXX")
  cat >"$tmp"
  chmod "$2" "$tmp"
  if [[ -f $1 ]] && cmp -s "$tmp" "$1"; then rm -f "$tmp"; return 1; fi
  mv "$tmp" "$1"
}

# Creates a 256-bit URL-safe secret once; an existing one is never rotated.
secret_file() { [[ -s $1 ]] || (umask 077 && openssl rand -hex 32 >"$1"); }

db_url() { printf 'postgresql://temps:%s@127.0.0.1:%s/temps?sslmode=disable' "$(<"$ETC/db-password")" "$DB_PORT"; }

ensure_docker() {
  local installed=false pkgs=()
  command -v docker >/dev/null || pkgs+=(docker.io docker-compose-v2)
  # nft lets the platform block workload egress to cloud metadata.
  command -v nft >/dev/null || pkgs+=(nftables)
  # The development build exports the binary with `docker build --output`, which needs BuildKit.
  if [[ $env == development ]] && ! docker buildx version >/dev/null 2>&1; then pkgs+=(docker-buildx); fi
  if ((${#pkgs[@]})) && command -v apt-get >/dev/null; then
    step "Installing distro packages: ${pkgs[*]}"
    DEBIAN_FRONTEND=noninteractive apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${pkgs[@]}" >/dev/null
    [[ " ${pkgs[*]} " == *" docker.io "* ]] && installed=true
  fi
  # Bound container logs on small disks, but only on a daemon this run installed.
  if $installed && [[ ! -e /etc/docker/daemon.json ]]; then
    install -d -m 0755 /etc/docker
    printf '{\n  "log-driver": "json-file",\n  "log-opts": {"max-size": "10m", "max-file": "3"}\n}\n' >/etc/docker/daemon.json
    systemctl restart docker
  fi
  systemctl enable --now docker >/dev/null 2>&1 || die "could not start docker.service"
  timeout 60 bash -c 'until docker info >/dev/null 2>&1; do sleep 1; done' ||
    die "the Docker daemon was not ready within 60s"
}

ensure_layout() {
  id "$BRAND" >/dev/null 2>&1 || useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin "$BRAND"
  # Docker socket access is root-equivalent; the platform needs it to run workloads.
  usermod -aG docker "$BRAND"
  install -d -m 0750 -o "$BRAND" -g "$BRAND" "$DATA" "$DATA/data"
  install -d -m 0700 "$ETC"
  touch "$MARKER"
  printf 'env=%s\nmode=%s\ndomain=%s\nemail=%s\n' "$env" "$mode" "$domain" "$email" |
    write_if_changed "$CONF" 0600 || true
}

ensure_db() {
  if docker volume inspect "$DB_VOLUME" >/dev/null 2>&1 && [[ ! -s $ETC/db-password ]]; then
    die "volume $DB_VOLUME holds a database but $ETC/db-password is missing; refusing to create a new password for existing data"
  fi
  secret_file "$ETC/db-password"
  # --env-file keeps the password out of every process argument list.
  printf 'POSTGRES_USER=temps\nPOSTGRES_DB=temps\nPOSTGRES_PASSWORD=%s\n' "$(<"$ETC/db-password")" |
    write_if_changed "$ETC/db.env" 0600 || true
  if docker inspect "$DB_CONTAINER" >/dev/null 2>&1; then
    docker start "$DB_CONTAINER" >/dev/null
  else
    step "Starting TimescaleDB ($DB_IMAGE) on 127.0.0.1:$DB_PORT"
    docker run -d --name "$DB_CONTAINER" --restart unless-stopped --shm-size=256m \
      --log-opt max-size=10m --log-opt max-file=3 --env-file "$ETC/db.env" \
      -p "127.0.0.1:$DB_PORT:5432" -v "$DB_VOLUME:/home/postgres/pgdata/data" \
      "$DB_IMAGE" >/dev/null
  fi
  # The image restarts Postgres once during first init: wait for 5 consecutive
  # successful probes so setup does not land in that window.
  step "Waiting for the database"
  local streak=0 deadline=$((SECONDS + 180))
  while ((streak < 5)); do
    ((SECONDS < deadline)) || die "database not ready within 180s (docker logs $DB_CONTAINER)"
    if docker exec "$DB_CONTAINER" pg_isready -U temps -q 2>/dev/null; then streak=$((streak + 1)); else streak=0; fi
    sleep 1
  done
}

# Both sources produce the binary in a staging dir and rename it into place: a
# failed download or build leaves the current binary untouched, and a rename
# never hits "text file busy" while the old binary is still running.
install_binary() {
  install -d -m 0755 "$PREFIX/bin"
  rm -rf "$PREFIX/staging"
  if [[ $env == development ]]; then build_from_source; else download_release; fi
  rm -rf "$PREFIX/staging"
  write_cli
}

download_release() {
  local args=() scratch f
  step "Downloading the release binary (SHA-256 verified by scripts/install.sh)"
  # Like deploy.sh: stable trails far behind the beta line this repository tracks.
  [[ -n $version || -n $channel ]] || channel=beta
  [[ -n $channel ]] && args+=(--channel "$channel")
  [[ -n $version ]] && args+=("$version")
  scratch=$(mktemp -d)
  # HOME is scratch so the installer's shell-rc PATH edits never touch root's dotfiles.
  HOME=$scratch TEMPS_INSTALL=$PREFIX/staging bash "$BINARY_INSTALLER" "${args[@]}"
  for f in "$PREFIX"/staging/bin/*; do mv -f "$f" "$PREFIX/bin/"; done
  rm -rf "$scratch"
}

# Builds this checkout (the fork's own code) with the repository Dockerfile, so
# the host needs no Rust or Bun. The `fast` profile and BuildKit's cargo cache
# mount make a rebuild after a code change take minutes, not the first build's
# long wait.
build_from_source() {
  local out=$PREFIX/staging
  step "Building this checkout with Docker (the first build takes a while; rebuilds reuse the cache)"
  docker build --target artifacts-source --build-arg TEMPS_BUILD_PROFILE=fast \
    --output "type=local,dest=$out" -f "$REPO_ROOT/Dockerfile" "$REPO_ROOT"
  chmod 0755 "$out/temps"
  mv -f "$out/temps" "$PREFIX/bin/temps"
  # The build ships both GeoLite2 databases; the service reads them from its data dir.
  install -m 0644 -o "$BRAND" -g "$BRAND" "$out"/GeoLite2-*.mmdb "$DATA/data/"
}

# The $BRAND command (and `temps`, unless something else owns that name) runs the
# binary with this server's settings, so `sudo $BRAND reset-admin-password`,
# `doctor` or `migrate` find the database without putting it on a command line.
write_cli() {
  cat <<EOF | write_if_changed "/usr/local/bin/$BRAND" 0755 || true
#!/bin/sh
# Generated by the $BRAND installer: the platform binary with this server's settings.
if [ "\$(id -u)" = 0 ] && [ -r $ETC/temps.env ]; then
  set -a; . $ETC/temps.env; set +a
  export TEMPS_DATA_DIR=$DATA/data
  # Run as the service user so files it writes in the data dir stay readable by the service.
  exec runuser -u $BRAND -- $PREFIX/bin/temps "\$@"
fi
exec $PREFIX/bin/temps "\$@"
EOF
  if [[ ! -e /usr/local/bin/temps || $(readlink /usr/local/bin/temps) == "/usr/local/bin/$BRAND" ]]; then
    ln -sfn "/usr/local/bin/$BRAND" /usr/local/bin/temps
  fi
}

ensure_binary() {
  if [[ -x $PREFIX/bin/temps ]]; then
    ok "$("$PREFIX/bin/temps" --version | head -1) kept (change or rebuild it with --upgrade)"
    return
  fi
  install_binary
}

wait_ready() { timeout "$1" bash -c "until curl -fs -o /dev/null http://$CONSOLE_ADDR/readyz; do sleep 2; done"; }

ensure_setup() {
  if [[ -f $SETUP_DONE ]]; then ok "setup already applied (kept)"; return; fi
  step "Running setup (admin account, encryption keys, migrations, domain)"
  local out args=(setup --auto --admin-email "$email" --admin-name "$first_name $last_name"
    --data-dir "$DATA/data" --wildcard-domain "*.$domain" --output-format json)
  [[ $mode == public ]] && args+=(--letsencrypt-email "$email")
  out=$(mktemp "$ETC/setup-output.XXXXXX")
  # Secrets travel in the environment (runuser keeps it), never in argv. The JSON
  # output echoes the admin password, so it is deleted instead of kept or logged.
  if ! TEMPS_DATABASE_URL=$(db_url) TEMPS_ADMIN_PASSWORD=$password \
    runuser -u "$BRAND" -- "$PREFIX/bin/temps" "${args[@]}" >"$out"; then
    rm -f "$out"
    die "setup failed (output above)"
  fi
  rm -f "$out"
  password=""
  touch "$SETUP_DONE"
}

ensure_service() {
  local restart=false
  printf 'TEMPS_DATABASE_URL=%s\n' "$(db_url)" | write_if_changed "$ETC/temps.env" 0600 && restart=true
  if sed -e "s|@BRAND@|$BRAND|g" -e "s|@PREFIX@|$PREFIX|g" -e "s|@DATA@|$DATA|g" -e "s|@ETC@|$ETC|g" \
    -e "s|@HTTP_ADDR@|$HTTP_ADDR|g" -e "s|@TLS_ADDR@|$TLS_ADDR|g" -e "s|@LOG_LEVEL@|$LOG_LEVEL|g" \
    -e "s|@DISABLE_HTTPS_REDIRECT@|$DISABLE_HTTPS_REDIRECT|g" \
    -e "s|@CONSOLE_ADDR@|$CONSOLE_ADDR|g" "$UNIT_TEMPLATE" | write_if_changed "$UNIT" 0644; then
    systemctl daemon-reload
    restart=true
  fi
  # Re-enabling also creates the $BRAND.service alias declared in [Install].
  systemctl reenable temps >/dev/null 2>&1
  if $restart && systemctl is-active --quiet temps; then systemctl restart temps; else systemctl start temps; fi

  step "Waiting for http://$CONSOLE_ADDR/readyz"
  if ! wait_ready 180; then
    journalctl -u temps -n 40 --no-pager >&2 || true
    die "the platform did not become ready within 180s"
  fi
}

upgrade() {
  [[ -f $SETUP_DONE && -x $PREFIX/bin/temps ]] || die "no $BRAND install from this installer here: run it without --upgrade first"
  local old new backup
  old=$("$PREFIX/bin/temps" --version 2>/dev/null | head -1 || true)
  old=${old:-the previous binary}
  ensure_docker
  docker start "$DB_CONTAINER" >/dev/null
  install -d -m 0700 "$DATA/backups"
  backup=$DATA/backups/pre-upgrade-$(date -u +%Y%m%dT%H%M%SZ).dump
  step "Backing up the database to $backup"
  if ! (umask 077 && docker exec "$DB_CONTAINER" pg_dump -U temps -d temps --format=custom >"$backup"); then
    rm -f "$backup"
    die "the database backup failed; nothing was changed"
  fi
  # Keep the three newest backups.
  ls -1t "$DATA"/backups/pre-upgrade-*.dump | tail -n +4 | xargs -r rm -f --
  cp -a "$PREFIX/bin/temps" "$PREFIX/bin/temps.previous"
  install_binary
  new=$("$PREFIX/bin/temps" --version 2>/dev/null | head -1 || true)
  new=${new:-a binary that cannot report its version}
  step "Restarting on $new"
  systemctl restart temps
  if wait_ready 120; then
    ok "upgraded from $old to $new; the previous binary is kept as $PREFIX/bin/temps.previous"
    return
  fi

  warn "$new did not become ready within 120s; rolling back to $old"
  journalctl -u temps -n 20 --no-pager || true
  mv -f "$PREFIX/bin/temps.previous" "$PREFIX/bin/temps"
  systemctl restart temps
  # Restoring the database would discard data, so it stays a decision for the operator.
  wait_ready 180 || die "the rollback did not recover either. Database backup from before the upgrade: $backup (restore with temps stopped: docker exec -i $DB_CONTAINER pg_restore -U temps -d temps --clean --if-exists < $backup)"
  die "the upgrade to $new failed and was rolled back: $old is serving again (database backup: $backup)"
}

uninstall() {
  [[ $EUID -eq 0 ]] || die "run as root (sudo)"
  managed || die "nothing to uninstall: $MARKER is missing, so this installer did not set up this host"
  if $purge; then
    [[ -t 0 ]] || die "--purge deletes the database and keys: run it in a terminal to confirm"
    local reply
    read -rp "This deletes the $BRAND database, keys and data on this host. Type $BRAND to confirm: " reply
    [[ $reply == "$BRAND" ]] || die "not confirmed; nothing was removed"
  fi
  step "Removing the $BRAND service, binary and database container"
  systemctl disable --now temps >/dev/null 2>&1 || true
  rm -f "$UNIT" "/etc/systemd/system/$BRAND.service"
  systemctl daemon-reload
  if command -v docker >/dev/null; then docker rm -f "$DB_CONTAINER" >/dev/null 2>&1 || true; fi
  rm -rf "$PREFIX" "/usr/local/bin/$BRAND"
  if [[ $(readlink /usr/local/bin/temps 2>/dev/null) == "/usr/local/bin/$BRAND" ]]; then rm -f /usr/local/bin/temps; fi
  if $purge; then
    if command -v docker >/dev/null; then docker volume rm "$DB_VOLUME" >/dev/null 2>&1 || true; fi
    rm -rf "$ETC" "$DATA" "$LOG"
    userdel "$BRAND" 2>/dev/null || true
    ok "purged the database volume, keys, data and the $BRAND user"
  else
    ok "kept $DB_VOLUME, $ETC and $DATA: re-run the installer to bring $BRAND back, or add --purge to delete them"
  fi
  local left=0
  command -v docker >/dev/null && left=$( (docker ps -aq --filter label=sh.temps.kind 2>/dev/null || true) | wc -l)
  ((left == 0)) || warn "$left app or service containers deployed through $BRAND were left alone (docker ps --filter label=sh.temps.kind)"
}

summary() {
  local scheme=https
  [[ $mode == local ]] && scheme=http
  cat <<EOF

$BRAND is running ($env, $mode mode, $LOG_LEVEL logs).
  Console         $scheme://console.$domain
  Admin           ${first_name:+$first_name $last_name, }$email (the password you chose)
  Health          curl -fsS http://$CONSOLE_ADDR/readyz
  Service         systemctl status $BRAND   |   journalctl -u temps -f
  CLI             sudo $BRAND --help
  Install log     $LOG
EOF
  if [[ $env == development ]]; then
    printf '  Your changes    rebuild and restart with: sudo %s --upgrade\n' "${BASH_SOURCE[0]}"
  fi
  [[ $mode == local ]] || printf '  Network         open TCP 80 and 443; point %s and *.%s at this host (sslip.io does it for you)\n' \
    "$domain" "$domain"
}

if $uninstall; then uninstall; exit 0; fi
choose_env
preflight
if $check_only; then echo "Preflight passed; nothing was changed."; exit 0; fi
$upgrade || gather_inputs

touch "$LOG" && chmod 600 "$LOG"
exec > >(tee -a "$LOG") 2>&1
trap 'rc=$?; ((rc == 0)) || printf "\nFailed (exit %s). Full log: %s. Re-running is safe.\n" "$rc" "$LOG" >&2' EXIT
printf '\n--- %s %s %s (env=%s mode=%s) ---\n' "$BRAND" "$($upgrade && echo upgrade || echo install)" \
  "$(date -u +%FT%TZ)" "$env" "$mode"

if $upgrade; then upgrade; exit 0; fi
ensure_docker
ensure_layout
ensure_db
ensure_binary
ensure_setup
ensure_service
summary
