#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2024-2026 Temps Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# Read-only check of what the start-temps skill needs to build and run Temps
# from this checkout (dev mode), including WSL2 pitfalls. Prints the fix for
# every gap and changes nothing. Exit 0 means start-temps can run.
#   .agents/skills/fuck-install/scripts/dev-check.sh
set -uo pipefail

REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)
gaps=0 wsl=false
ok() { printf '  ok    %s\n' "$*"; }
note() { printf '  note  %s\n' "$*"; }
gap() { printf '  MISS  %s\n        fix: %s\n' "$1" "$2"; gaps=$((gaps + 1)); }

echo "==> Dev prerequisites for start-temps"
case $(uname -s) in
  Linux)
    if grep -qi microsoft /proc/version 2>/dev/null; then wsl=true; ok "WSL2"; else ok "Linux"; fi ;;
  Darwin) ok "macOS" ;;
  *)
    gap "native Windows shell" "Temps (pingora) builds on Linux and macOS only: open a WSL2 shell and run this there"
    exit 1 ;;
esac
if $wsl && [[ $REPO_ROOT == /mnt/* ]]; then
  note "the checkout is on the Windows drive: builds run several times faster from a clone under ~/ inside WSL"
fi

want=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$REPO_ROOT/rust-toolchain.toml")
if command -v cargo >/dev/null && command -v rustup >/dev/null; then
  ok "cargo + rustup (rust-toolchain.toml pins $want)"
  rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown && ok "wasm32 target" ||
    gap "wasm32-unknown-unknown target" "rustup target add wasm32-unknown-unknown"
  command -v wasm-pack >/dev/null && ok "wasm-pack" || gap "wasm-pack" "cargo install wasm-pack --version 0.15.0 --locked"
else
  gap "Rust toolchain $want" "install rustup (https://rustup.rs); it installs $want from rust-toolchain.toml on first use"
fi

if command -v dpkg >/dev/null; then
  pkgs=()
  for p in build-essential cmake pkg-config libssl-dev protobuf-compiler lsof; do
    dpkg -s "$p" >/dev/null 2>&1 || pkgs+=("$p")
  done
  ((${#pkgs[@]} == 0)) && ok "build packages (CONTRIBUTING.md)" ||
    gap "packages: ${pkgs[*]}" "sudo apt-get install -y ${pkgs[*]}"
else
  for c in cmake protoc pkg-config lsof; do
    command -v "$c" >/dev/null && ok "$c" || gap "$c" "macOS: brew install cmake openssl pkg-config protobuf"
  done
fi

bun_path=$(command -v bun || true)
if [[ -z $bun_path ]]; then
  gap "bun" "install Bun (https://bun.sh), then: cd web && bun install"
elif $wsl && [[ $bun_path == /mnt/* ]]; then
  gap "bun resolves to the Windows binary ($bun_path)" "install Bun inside WSL (https://bun.sh) so it comes first on PATH"
else
  ok "bun $(bun --version)"
fi
[[ -d $REPO_ROOT/web/node_modules ]] && ok "web/node_modules" || gap "web dependencies" "cd web && bun install"

if docker info >/dev/null 2>&1; then
  ok "Docker daemon"
  db=$(docker ps --format '{{.Names}} {{.Image}}' | awk '$2 ~ /timescale/ {print $1; exit}')
  if [[ -z $db ]]; then
    gap "TimescaleDB container" "run the docker command under Prerequisites in .agents/skills/start-temps/SKILL.md"
  elif [[ $db != temps-db ]]; then
    ok "TimescaleDB container $db"
    note "start-temps expects a container named temps-db: export TEMPS_DEV_DB_CONTAINER=$db first"
  else
    ok "TimescaleDB container temps-db"
  fi
else
  gap "Docker daemon" "start Docker (WSL2: docker.io inside WSL, or Docker Desktop with WSL integration)"
fi

# An installed Temps (not a checkout build) usually owns slot 0's ports and the
# shared `temps` database; start-temps then has to pick another slot.
while read -r pid; do
  [[ -n $pid ]] || continue
  exe=$(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | head -1)
  [[ $exe == */target/* ]] && continue
  note "an installed Temps is running (pid $pid, $exe): start-temps skips busy slots, but never point this checkout at database 'temps'"
done < <(pgrep -x temps 2>/dev/null || true)

echo
if ((gaps)); then
  echo "$gaps gap(s): fix them, re-run this check, then follow .agents/skills/start-temps/SKILL.md"
  exit 1
fi
echo "Ready: follow .agents/skills/start-temps/SKILL.md (or ask your agent to 'start temps')."
