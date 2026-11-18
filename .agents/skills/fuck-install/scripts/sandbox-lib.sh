#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2024-2026 Temps Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# Shared by test-prod-sandbox.sh and devbox.sh: the image of a fresh systemd
# "server" and the copy of the checkout into a box. Source it; it only defines.

SANDBOX_IMAGE=temps-install-sandbox:ubuntu24.04

# Builds (or reuses from cache) a minimal Ubuntu 24.04 image that boots systemd,
# so a privileged container behaves like a fresh server. socat lets the devbox
# publish services that bind the box's loopback.
build_sandbox_image() {
  docker build -q -t "$SANDBOX_IMAGE" - >/dev/null <<'EOF'
FROM ubuntu:24.04
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      systemd systemd-sysv dbus ca-certificates curl openssl iproute2 procps socat >/dev/null \
 && rm -rf /var/lib/apt/lists/* \
 && systemctl mask systemd-logind.service getty.target console-getty.service
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
EOF
}

# Waits until systemd in box $1 has finished booting.
wait_for_systemd() {
  timeout 90 bash -c "until docker exec $1 systemctl is-system-running 2>/dev/null | grep -qE 'running|degraded'; do sleep 1; done"
}

# ship_checkout BOX ROOT PATH...: replaces /src in BOX with PATHs of the checkout
# at ROOT, as LF like a Linux clone. A Windows checkout has CRLF; only text
# files are rewritten, so binaries survive.
# SHIP_DEST picks the directory in the box (default /src).
ship_checkout() {
  local box=$1 root=$2 dest=${SHIP_DEST:-/src}
  shift 2
  # tar exits 1 when a file changes while it is read (an editor saving):
  # the copy is still usable, so only a real failure (2) stops the ship.
  { tar -C "$root" --exclude=.git --exclude=target --exclude=node_modules \
      --warning=no-file-changed -cf - "$@" || [ $? -eq 1 ]; } |
    docker exec -i -e DEST="$dest" "$box" sh -c 'rm -rf "$DEST" && mkdir -p "$DEST" && tar -C "$DEST" -xf - &&
      find "$DEST" -type f -size -4M -exec grep -IlZ "$(printf "\r")" {} + | xargs -0 -r sed -i "s/\r\$//"'
}
