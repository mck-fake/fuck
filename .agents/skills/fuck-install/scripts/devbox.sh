#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2024-2026 Temps Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# The development environment on this machine, in a box of its own: a systemd
# container with its own Docker daemon, running install.sh --env development.
# It shares no ports, unit files, containers or networks with anything else
# here, which matters because the platform uses fixed Docker names
# (temps-preview-gateway, temps-app-network) that a second instance on the same
# daemon would fight over. Needs Docker access only: no sudo, Rust or Bun.
#
#   devbox.sh up [installer flags]  create or start the box, build this checkout, install
#   devbox.sh dev                   fast loop: sync changed files, incremental debug build, restart
#   devbox.sh rebuild               copy the checkout in again and rebuild (install.sh --upgrade)
#   devbox.sh test <cargo test args> run cargo test on this checkout, e.g. test -p temps-auth --lib
#   devbox.sh status                service state and URLs
#   devbox.sh logs                  follow the platform's logs
#   devbox.sh shell                 root shell inside the box
#   devbox.sh down                  stop the box; data and build cache are kept
#   devbox.sh destroy               delete the box, its data and build cache (asks first)
set -euo pipefail

SKILL_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
REPO_ROOT=$(cd "$SKILL_DIR/../../.." && pwd)
SKILL_REL=${SKILL_DIR#"$REPO_ROOT"/}
# shellcheck source=sandbox-lib.sh
. "$SKILL_DIR/scripts/sandbox-lib.sh"
BRAND=$(sed -n 's/^BRAND=//p' "$SKILL_DIR/scripts/install.sh")
BOX=$BRAND-devbox
CONSOLE_PORT=9081
# Not 9080: audio software (Nahimic) commonly holds 9080 on Windows, which then
# shadows WSL's localhost forwarding for that port.
HTTP_PORT=9180
INSTALL="bash /src/$SKILL_REL/scripts/install.sh"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
usage() { sed -n '5,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
exists() { docker container inspect "$BOX" >/dev/null 2>&1; }
running() { [[ $(docker container inspect -f '{{.State.Running}}' "$BOX" 2>/dev/null) == true ]]; }
# Interactive only when this shell has a terminal, so the installer can ask its questions.
exec_flags() { if [[ -t 0 && -t 1 ]]; then echo -it; else echo -i; fi; }

start_box() {
  build_sandbox_image
  if exists; then
    docker start "$BOX" >/dev/null
  else
    docker run -d --name "$BOX" --hostname "$BOX" --privileged --cgroupns=private \
      --restart unless-stopped --tmpfs /run --tmpfs /run/lock \
      -v "$BOX-docker:/var/lib/docker" -v "$BOX-containerd:/var/lib/containerd" \
      -v "$BOX-etc:/etc/$BRAND" -v "$BOX-data:/var/lib/$BRAND" \
      -p "127.0.0.1:$CONSOLE_PORT:18081" -p "127.0.0.1:$HTTP_PORT:18080" \
      "$SANDBOX_IMAGE" >/dev/null
  fi
  wait_for_systemd "$BOX"
  # The platform binds the box's loopback; these forwarders expose it on the
  # box's interface, which Docker publishes only on this machine's loopback.
  local pair
  for pair in 18081:8081 18080:80; do
    docker exec -i "$BOX" sh -c "cat > /etc/systemd/system/devbox-forward-${pair%%:*}.service" <<EOF
[Unit]
Description=Publish 127.0.0.1:${pair#*:} on port ${pair%%:*} for the devbox
[Service]
ExecStart=/usr/bin/socat TCP-LISTEN:${pair%%:*},fork,reuseaddr TCP:127.0.0.1:${pair#*:}
Restart=always
[Install]
WantedBy=multi-user.target
EOF
  done
  docker exec "$BOX" sh -c 'systemctl daemon-reload && systemctl enable --now devbox-forward-18081 devbox-forward-18080' >/dev/null 2>&1
}

cmd_up() {
  local args=() password_file=""
  while (($#)); do
    case $1 in
      --password-file)
        [[ -r ${2:-} ]] || die "cannot read --password-file ${2:-}"
        password_file=$2
        shift 2
        ;;
      *) args+=("$1"); shift ;;
    esac
  done
  start_box
  echo "==> Copying this checkout into $BOX"
  ship_checkout "$BOX" "$REPO_ROOT" .
  if [[ -n $password_file ]]; then
    docker exec -i "$BOX" sh -c 'umask 077; cat > /root/admin-password' <"$password_file"
    args+=(--password-file /root/admin-password)
  fi
  local status=0
  # shellcheck disable=SC2086 # INSTALL is "bash <path>" on purpose
  docker exec $(exec_flags) "$BOX" $INSTALL --env development "${args[@]}" || status=$?
  # Removed on failure too: the password must not outlive the install.
  docker exec "$BOX" rm -f /root/admin-password
  ((status == 0)) || exit "$status"
  cmd_status
}

cmd_rebuild() {
  running || die "the devbox is not running: $0 up"
  echo "==> Copying this checkout into $BOX"
  ship_checkout "$BOX" "$REPO_ROOT" .
  # shellcheck disable=SC2086
  docker exec $(exec_flags) "$BOX" $INSTALL --upgrade
  cmd_status
}

# The repository's toolchain stage (cached) plus static OpenSSL, which
# test-only dev-dependencies (testcontainers' native-tls) link on musl; the
# release build never needs it, so it lives here. $1 is a checkout in the box.
ensure_build_image() {
  docker exec "$BOX" docker build -q --target toolchain -t "$BOX-toolchain" -f "$1/Dockerfile" "$1" >/dev/null
  printf 'FROM %s-toolchain\nRUN apk add --no-cache openssl-libs-static\n' "$BOX" |
    docker exec -i "$BOX" docker build -q -t "$BOX-test" - >/dev/null
}

# The fast loop for Rust changes: a persistent copy synced by content (so
# unchanged files keep their mtimes and cargo recompiles only what changed),
# an incremental debug build, the web UI rebuilt only when web/ changed, then
# the binary swapped in place. No backup and no rollback: development only;
# `rebuild` stays the production-like path.
DEV_SRC=/src-dev
cmd_dev() {
  running || die "the devbox is not running: $0 up"
  local stage=/src-stage-$$ started=$SECONDS
  echo "==> Syncing changed files into $BOX:$DEV_SRC"
  SHIP_DEST=$stage ship_checkout "$BOX" "$REPO_ROOT" .
  docker exec "$BOX" sh -c 'command -v rsync >/dev/null || { apt-get update -qq && apt-get install -y -qq rsync >/dev/null; }'
  docker exec "$BOX" rsync -rc --delete --exclude node_modules --exclude /target \
    --exclude /crates/temps-cli/dist --exclude /crates/temps-captcha-wasm/pkg --exclude /.web-hash \
    "$stage/" "$DEV_SRC/"
  docker exec "$BOX" rm -rf "$stage"
  ensure_build_image "$DEV_SRC"
  docker exec "$BOX" mkdir -p /var/tmp/devbox-out
  # shellcheck disable=SC2046
  docker exec $(exec_flags) "$BOX" docker run --rm -v "$DEV_SRC:/build" -w /build \
    -v devbox-test-target:/build/target -v devbox-cargo-registry:/usr/local/cargo/registry \
    -v devbox-web-modules:/build/web/node_modules -v /var/tmp/devbox-out:/out \
    "$BOX-test" sh -euc '
      if [ ! -d crates/temps-captcha-wasm/pkg ]; then
        echo "==> Building the captcha WASM (first run only)"
        (cd crates/temps-captcha-wasm && bun install && npm run build)
      fi
      hash=$(find web/src web/public web/package.json web/bun.lock web/rsbuild.config.ts -type f -print0 |
        sort -z | xargs -0 sha256sum | sha256sum)
      if [ ! -f crates/temps-cli/dist/assets/.devbox ] || [ "$hash" != "$(cat .web-hash 2>/dev/null)" ]; then
        echo "==> web/ changed: rebuilding the embedded console"
        # A web build that needs a not-yet-regenerated SDK must not block the
        # backend it is waiting for: keep the previous console and say so.
        if (cd web && bun install --frozen-lockfile && RSBUILD_OUTPUT_PATH=/build/crates/temps-cli/dist bun run build) >/tmp/web-build.log 2>&1; then
          mkdir -p crates/temps-cli/dist/assets && touch crates/temps-cli/dist/assets/.devbox
          echo "$hash" > .web-hash
          # include_dir! does not track dist/, so make cargo recompile its user.
          touch crates/temps-cli/src/commands/serve/console.rs
        else
          grep -m3 -E "error|×" /tmp/web-build.log || tail -5 /tmp/web-build.log
          echo "WARNING: the console build failed; the previous console stays embedded."
          echo "         If it needs new API calls: regenerate the SDK from this server, then run dev again."
        fi
      fi
      echo "==> cargo build (debug, incremental)"
      SKIP_WEB_BUILD=1 cargo build --bin temps -p temps-cli
      cp target/debug/temps /out/temps'
  docker exec "$BOX" sh -c 'install -m 0755 /var/tmp/devbox-out/temps /opt/'"$BRAND"'/bin/temps.dev &&
    mv -f /opt/'"$BRAND"'/bin/temps.dev /opt/'"$BRAND"'/bin/temps && systemctl restart temps'
  local i code=""
  for i in $(seq 1 60); do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "http://127.0.0.1:$CONSOLE_PORT/readyz" || true)
    [[ $code == 200 ]] && break
    sleep 2
  done
  [[ $code == 200 ]] || die "the new binary did not become ready: $0 logs"
  echo "==> Running the new build ($((SECONDS - started))s)"
  cmd_status
}

# Tests run in the repository's own toolchain image (the Dockerfile's cached
# toolchain stage) on the box's daemon; target and registry live in volumes of
# the box's daemon so repeated runs compile incrementally. The box's own Docker
# socket and network let TestDatabase tests start their throwaway Postgres
# there, never on this machine's daemon.
cmd_test() {
  running || die "the devbox is not running: $0 up"
  (($#)) || die "usage: $0 test -p <crate> [--lib] [test name] [-- args]"
  # A copy per run, so concurrent runs never read each other's half-copied
  # tree; cargo's own lock serialises them on the shared target volume.
  local src=/src-test-$$ status=0
  echo "==> Copying this checkout into $BOX:$src"
  SHIP_DEST=$src ship_checkout "$BOX" "$REPO_ROOT" .
  ensure_build_image "$src"
  # shellcheck disable=SC2046
  docker exec $(exec_flags) "$BOX" docker run --rm -v "$src:/build" -w /build \
    --network host -v /var/run/docker.sock:/var/run/docker.sock \
    -v devbox-test-target:/build/target -v devbox-cargo-registry:/usr/local/cargo/registry \
    "$BOX-test" cargo test "$@" || status=$?
  docker exec "$BOX" rm -rf "$src"
  return "$status"
}

cmd_status() {
  if ! exists; then echo "No devbox yet: $0 up"; return; fi
  if ! running; then echo "The devbox is stopped: $0 up"; return; fi
  local state ready
  state=$(docker exec "$BOX" systemctl is-active temps 2>/dev/null || true)
  ready=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$CONSOLE_PORT/readyz" || true)
  cat <<EOF

$BOX: service ${state:-not installed}, /readyz ${ready:-unreachable}
  Console   http://localhost:$CONSOLE_PORT
  Apps      http://<name>.127-0-0-1.sslip.io:$HTTP_PORT
  Rebuild   $0 rebuild     Logs   $0 logs     Shell   $0 shell
EOF
}

cmd_destroy() {
  [[ -t 0 ]] || die "destroy deletes the devbox database and build cache: run it in a terminal to confirm"
  local reply
  read -rp "Delete $BOX with its database and build cache? Type $BOX to confirm: " reply
  [[ $reply == "$BOX" ]] || die "not confirmed; nothing was removed"
  docker rm -f "$BOX" >/dev/null 2>&1 || true
  docker volume rm "$BOX-docker" "$BOX-containerd" "$BOX-etc" "$BOX-data" >/dev/null 2>&1 || true
  echo "Deleted $BOX and its volumes."
}

case ${1:-} in
  -h | --help | "") usage; exit 0 ;;
esac
docker info >/dev/null 2>&1 || die "Docker is not reachable from this shell"
case $1 in
  up) shift; cmd_up "$@" ;;
  rebuild) cmd_rebuild ;;
  test) shift; cmd_test "$@" ;;
  dev) cmd_dev ;;
  status) cmd_status ;;
  logs) running || die "the devbox is not running: $0 up"; docker exec $(exec_flags) "$BOX" journalctl -u temps -n 100 -f ;;
  shell) running || die "the devbox is not running: $0 up"; docker exec -it "$BOX" bash ;;
  down) docker stop "$BOX" >/dev/null && echo "Stopped $BOX; data and build cache are kept." ;;
  destroy) cmd_destroy ;;
  *) die "unknown command: $1 (see --help)" ;;
esac
