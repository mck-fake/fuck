#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2024-2026 Temps Contributors
# SPDX-License-Identifier: MIT OR Apache-2.0

# End-to-end test of scripts/install.sh on a stand-in for a fresh Ubuntu 24.04
# server: a disposable privileged systemd container with its own Docker daemon.
# Covers the input gates, the interactive questions (answered through a pty),
# health, security, an idempotent re-run, a restart, a real upgrade, the
# automatic rollback from a broken release, uninstall/reinstall and purge. It
# creates one uniquely named container (plus its anonymous volumes) and the
# reusable image below, and removes only those. The inner Docker's stores
# (/var/lib/docker, /var/lib/containerd) are volumes because overlayfs cannot
# nest on the container's own overlay root.
#   test-prod-sandbox.sh [--keep]                    # production install scenario
#   test-prod-sandbox.sh --env development [--keep]  # builds this checkout in the sandbox (long)
# --keep leaves the container for inspection.
set -euo pipefail

SKILL_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
REPO_ROOT=$(cd "$SKILL_DIR/../../.." && pwd)
SKILL_REL=${SKILL_DIR#"$REPO_ROOT"/}
# shellcheck source=sandbox-lib.sh
. "$SKILL_DIR/scripts/sandbox-lib.sh"
SCRIPT=/src/$SKILL_REL/scripts/install.sh
INSTALL="bash $SCRIPT"
BRAND=$(sed -n 's/^BRAND=//p' "$SKILL_DIR/scripts/install.sh")
# The first install pins an older beta so the upgrade test moves to a newer one.
OLD_VERSION=v0.1.0-beta.57
NAME=$BRAND-install-test-$(date +%s)-$$
KEEP=false ENV_UNDER_TEST=production
while (($#)); do
  case $1 in
    --keep) KEEP=true; shift ;;
    --env) ENV_UNDER_TEST=${2:-}; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ $ENV_UNDER_TEST == production || $ENV_UNDER_TEST == development ]] || { echo "--env is production or development" >&2; exit 2; }
passed=0 failed=0

box() { docker exec -e BRAND="$BRAND" "$NAME" bash -c "$1"; }
check() {
  if box "$2" >/dev/null 2>&1; then
    printf '  PASS  %s\n' "$1"; passed=$((passed + 1))
  else
    printf '  FAIL  %s\n' "$1"; failed=$((failed + 1))
  fi
}
same() { # label before after
  if [[ $2 == "$3" ]]; then printf '  PASS  %s\n' "$1"; passed=$((passed + 1))
  else printf '  FAIL  %s\n' "$1"; failed=$((failed + 1)); fi
}
cleanup() {
  if $KEEP; then echo "Kept $NAME (remove with: docker rm -f -v $NAME)"
  else docker rm -f -v "$NAME" >/dev/null 2>&1 || true; fi
}
SECRETS='sha256sum "/etc/$BRAND/db-password" "/etc/$BRAND/temps.env" "/etc/$BRAND/install.conf"'
READY='curl -fsS -o /dev/null http://127.0.0.1:8081/readyz'
LOGIN='for p in /api/auth/login /auth/login; do [ "$(curl -s -o /dev/null -w "%{http_code}" -H "Content-Type: application/json" --data @/root/login.json "http://127.0.0.1:8081$p")" = 200 ] && exit 0; done; exit 1'

if ! docker info >/dev/null 2>&1; then
  echo "SKIPPED: Docker is not reachable from this shell."
  exit 0
fi

echo "==> Building $SANDBOX_IMAGE (cached after the first run)"
build_sandbox_image

trap cleanup EXIT
echo "==> Booting sandbox $NAME"
docker run -d --name "$NAME" --hostname sandbox --privileged --cgroupns=private \
  --tmpfs /run --tmpfs /run/lock -v /var/lib/docker -v /var/lib/containerd "$SANDBOX_IMAGE" >/dev/null
wait_for_systemd "$NAME"
# Production needs only the installers; development builds the whole checkout.
if [[ $ENV_UNDER_TEST == development ]]; then
  ship_checkout "$NAME" "$REPO_ROOT" .
else
  ship_checkout "$NAME" "$REPO_ROOT" scripts/install.sh "$SKILL_REL"
fi
# A password that meets the policy, kept in root-only files for the checks below.
box 'umask 077; printf "Aa1!%s\n" "$(openssl rand -hex 12)" > /root/pw; printf "{\"email\":\"admin@example.com\",\"password\":\"%s\"}" "$(cat /root/pw)" > /root/login.json'

if [[ $ENV_UNDER_TEST == development ]]; then
  echo "==> Development install, answering the questions through a terminal (builds this checkout)"
  # Questions: environment, email, first name, last name, password twice (no domain on loopback).
  answers='printf "%s\n" development admin@example.com Ada Lovelace "$(cat /root/pw)" "$(cat /root/pw)"'
  if ! box "$answers | timeout 5400 script -qec '$INSTALL' /dev/null" 2>&1 | grep -v -F -f <(box 'cat /root/pw') | sed 's/^/    /'; then
    box 'journalctl -u temps -n 60 --no-pager' 2>&1 | sed 's/^/    /' || true
    echo "FAIL: development install exited non-zero"
    exit 1
  fi

  echo "==> Development checks"
  check "service active, also through the \$BRAND alias" 'systemctl is-active --quiet temps && systemctl is-active --quiet "$BRAND"'
  check "console /readyz returns 200" "$READY"
  check "the chosen password signs in" "$LOGIN"
  check "the binary was built from this checkout, not downloaded" \
    'grep -q "Building this checkout" "/var/log/$BRAND-install.log" && ! grep -q "Downloading the release binary" "/var/log/$BRAND-install.log"'
  check "debug logging" 'journalctl -u temps --no-pager | grep -q " DEBUG "'
  check "proxy and console on loopback only" \
    '[ "$(ss -Htln "sport = :80" | awk "{print \$4}" | sort -u)" = 127.0.0.1:80 ] && [ "$(ss -Htln "sport = :8081" | awk "{print \$4}" | sort -u)" = 127.0.0.1:8081 ]'
  check "remembered as a development install" 'grep -qx env=development "/etc/$BRAND/install.conf" && grep -qx domain=127-0-0-1.sslip.io "/etc/$BRAND/install.conf"'
  check "both GeoLite2 databases come with the build" \
    '[ -s "/var/lib/$BRAND/data/GeoLite2-City.mmdb" ] && [ -s "/var/lib/$BRAND/data/GeoLite2-ASN.mmdb" ]'
  check "admin commands find this server's database" '"$BRAND" migrate --dry-run'
  check "no ERROR lines in the journal" '! journalctl -u temps --no-pager | grep -q " ERROR "'
  check "switching the host to production needs a reinstall" "! $INSTALL --env production </dev/null"

  echo "==> Rebuild after a code change (--upgrade reuses the build cache)"
  before=$(box "$SECRETS")
  check "rebuild exits 0" "$INSTALL --upgrade </dev/null"
  check "/readyz 200 after the rebuild" "$READY"
  check "pre-upgrade database backup exists" '[ -s "$(ls -1t /var/lib/$BRAND/backups/pre-upgrade-*.dump | head -1)" ]'
  same "rebuild keeps every secret" "$before" "$(box "$SECRETS")"

  echo "==> Evidence"
  box '"$BRAND" --version; systemctl status "$BRAND" --no-pager -n 0 | head -6; ps -o rss=,comm= -C temps | awk "{printf \"platform RSS %d MB\n\", \$1/1024}"; journalctl -u temps --no-pager | grep -cE " (ERROR|WARN) " | xargs echo "ERROR/WARN journal lines:"' 2>&1 | sed 's/^/    /'
  echo
  echo "$passed passed, $failed failed"
  ((failed == 0))
  exit 0
fi

echo "==> Input gates"
check "rejects an invalid --email" "! $INSTALL --check --mode local --email 'a;b@x.io'"
check "development refuses --mode public" "! $INSTALL --check --env development --mode public"
check "rejects a weak password before changing anything" \
  "printf 'weakpass\n' > /root/weak; ! $INSTALL --mode local --email a@b.io --first-name A --last-name B --password-file /root/weak </dev/null; [ ! -e /etc/\$BRAND ]"
check "unattended run without a password stops before changing anything" \
  "! $INSTALL --mode local --email a@b.io --first-name A --last-name B </dev/null; [ ! -e /etc/\$BRAND ]"
check "refuses a temps.service it did not create" \
  "touch /etc/systemd/system/temps.service; $INSTALL --check --mode local; rc=\$?; rm -f /etc/systemd/system/temps.service; [ \$rc -ne 0 ]"
check "Enter on the domain question falls back to <public-ip>.sslip.io" \
  "source <(sed -n '/^valid_domain()/p; /^ask()/,/^}/p; /^ask_domain()/,/^}/p' $SCRIPT); mode=public domain=; ask_domain </dev/null; [[ \$domain =~ ^[0-9]+-[0-9]+-[0-9]+-[0-9]+\.sslip\.io\$ ]]"
check "uninstall refuses a host this installer did not set up" "! $INSTALL --uninstall </dev/null"
box "$INSTALL --check --mode local" | sed 's/^/    /'

echo "==> First install of $OLD_VERSION, answering the questions through a terminal"
# Same order as the questions: environment (Enter keeps production), email, first
# name, last name, password twice (local mode asks no domain).
answers='printf "%s\n" "" admin@example.com Ada Lovelace "$(cat /root/pw)" "$(cat /root/pw)"'
if ! box "$answers | timeout 900 script -qec '$INSTALL --mode local --version $OLD_VERSION' /dev/null" 2>&1 | grep -v -F -f <(box 'cat /root/pw') | sed 's/^/    /'; then
  box 'journalctl -u temps -n 60 --no-pager' 2>&1 | sed 's/^/    /' || true
  echo "FAIL: first install exited non-zero"
  exit 1
fi

echo "==> Health"
check "service active, also through the \$BRAND alias" 'systemctl is-active --quiet temps && systemctl is-active --quiet "$BRAND"'
check "service enabled at boot" 'systemctl is-enabled --quiet temps'
check "console /readyz returns 200" "$READY"
check "console /healthz returns 200" 'curl -fsS -o /dev/null http://127.0.0.1:8081/healthz'
check "proxy answers HTTP on 127.0.0.1:80" 'curl -sS -o /dev/null http://127.0.0.1:80/'
check "CLI available as \$BRAND, on the pinned version" "\"\$BRAND\" --version | grep -qF '$OLD_VERSION'"
check "temps works as the same command" "temps --version | grep -qF '$OLD_VERSION'"
check "admin commands find this server's database (sudo \$BRAND migrate --dry-run)" '"$BRAND" migrate --dry-run'
check "database container running" '[ "$(docker inspect -f {{.State.Running}} "$BRAND-postgres")" = true ]'
check "database restarts with the host" '[ "$(docker inspect -f {{.HostConfig.RestartPolicy.Name}} "$BRAND-postgres")" = unless-stopped ]'
check "no ERROR lines in the journal" '! journalctl -u temps --no-pager | grep -q " ERROR "'
check "docker compose available for Compose deployments" 'docker compose version'

echo "==> The answers took effect"
check "admin is stored as Ada Lovelace" \
  '[ "$(docker exec "$BRAND-postgres" psql -U temps -d temps -tAc "select name from users where email = '"'"'admin@example.com'"'"'")" = "Ada Lovelace" ]'
check "the chosen password signs in" "$LOGIN"
check "local mode advertises no public address" 'grep -qx "domain=127-0-0-1.sslip.io" "/etc/$BRAND/install.conf"'
check "local mode serves apps over plain HTTP (no HTTPS redirect without a certificate)" \
  'grep -qx "Environment=TEMPS_DISABLE_HTTPS_REDIRECT=true" /etc/systemd/system/temps.service'
check "Enter on the environment question chose production, with info logs" \
  'grep -qx env=production "/etc/$BRAND/install.conf" && grep -qx "Environment=TEMPS_LOG_LEVEL=info" /etc/systemd/system/temps.service'

echo "==> Security"
check "database published on loopback only" 'docker port "$BRAND-postgres" 5432 | grep -qx 127.0.0.1:16432'
check "console/admin API on loopback only" '[ "$(ss -Htln "sport = :8081" | awk "{print \$4}" | sort -u)" = 127.0.0.1:8081 ]'
check "secret files are 0600" 'for f in temps.env db.env db-password install.conf; do [ "$(stat -c %a "/etc/$BRAND/$f")" = 600 ] || exit 1; done'
check "admin password never written to disk or the log" '! grep -rqaFf /root/pw "/etc/$BRAND" "/var/lib/$BRAND" "/var/log/$BRAND-install.log"'
check "world-readable unit holds no database URL" '! grep -q postgresql:// /etc/systemd/system/temps.service'
check "no password in any process argv" '! grep -qsaFf "/etc/$BRAND/db-password" /proc/[0-9]*/cmdline && ! grep -qsaFf /root/pw /proc/[0-9]*/cmdline'
check "workload egress to cloud metadata is blocked" '! journalctl -u temps --no-pager | grep -q "Cloud-metadata egress block is incomplete"'
check "runs as the unprivileged \$BRAND user" '[ "$(ps -o user= -p "$(systemctl show -p MainPID --value temps)")" = "$BRAND" ]'

echo "==> Idempotent re-run (no flags, no terminal: nothing is asked again)"
before=$(box "$SECRETS")
check "re-run exits 0" "$INSTALL </dev/null"
same "re-run keeps every secret" "$before" "$(box "$SECRETS")"
check "a version flag on an existing install points to --upgrade" "! $INSTALL --version v0.1.0-beta.61 </dev/null"
check "re-run keeps loopback binding" '[ "$(ss -Htln "sport = :80" | awk "{print \$4}" | sort -u)" = 127.0.0.1:80 ]'
check "/readyz 200 after re-run" "$READY"

echo "==> Restart"
box 'systemctl restart "$BRAND"'
check "/readyz 200 after systemctl restart \$BRAND" "timeout 120 bash -c 'until $READY; do sleep 2; done'"

echo "==> Upgrade to the newest beta"
before=$(box "$SECRETS")
check "upgrade exits 0" "$INSTALL --upgrade </dev/null"
check "binary moved off $OLD_VERSION" "! \"\$BRAND\" --version | grep -qF '$OLD_VERSION'"
check "pre-upgrade database backup exists and is root-only" \
  'f=$(ls -1t /var/lib/$BRAND/backups/pre-upgrade-*.dump | head -1); [ -s "$f" ] && [ "$(stat -c %a "$f")" = 600 ]'
check "previous binary kept for a manual rollback" '[ -x "/opt/$BRAND/bin/temps.previous" ]'
check "/readyz 200 after the upgrade" "$READY"
check "the chosen password still signs in" "$LOGIN"
same "upgrade keeps every secret" "$before" "$(box "$SECRETS")"

echo "==> An upgrade to a release that cannot start rolls back on its own"
version_now=$(box '"$BRAND" --version | head -1')
# Swap in a stub for scripts/install.sh that "installs" a binary which exits at once.
docker exec -i "$NAME" sh -c 'cp /src/scripts/install.sh /root/install.sh.real && cat > /src/scripts/install.sh' <<'EOF'
#!/usr/bin/env bash
mkdir -p "$TEMPS_INSTALL/bin"
printf '#!/bin/sh\necho "stub: this release cannot start" >&2\nexit 1\n' >"$TEMPS_INSTALL/bin/temps"
chmod 755 "$TEMPS_INSTALL/bin/temps"
EOF
check "the failed upgrade exits non-zero" "! $INSTALL --upgrade </dev/null"
box 'cp /root/install.sh.real /src/scripts/install.sh'
check "/readyz 200 again on the previous binary" "$READY"
check "the version is the one from before the failed upgrade" "[ \"\$(\"\$BRAND\" --version | head -1)\" = '$version_now' ]"

echo "==> Evidence"
box 'systemctl status "$BRAND" --no-pager -n 0 | head -6; echo; docker ps --format "table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}"; echo; ps -o rss=,comm= -C temps | awk "{printf \"platform RSS %d MB\n\", \$1/1024}"; docker stats --no-stream --format "{{.Name}} {{.MemUsage}}"; ls -l /var/lib/$BRAND/backups; journalctl -u temps --no-pager | grep -cE " (ERROR|WARN) " | xargs echo "ERROR/WARN journal lines:"' 2>&1 | sed 's/^/    /'

echo "==> Uninstall keeps the data; a re-run brings the platform back"
before=$(box "$SECRETS")
check "uninstall exits 0" "$INSTALL --uninstall </dev/null"
check "service, binary, CLI and DB container are gone" \
  '! systemctl cat temps >/dev/null 2>&1 && [ ! -e "/opt/$BRAND" ] && [ ! -e "/usr/local/bin/$BRAND" ] && [ ! -e /usr/local/bin/temps ] && ! docker inspect "$BRAND-postgres" >/dev/null 2>&1'
check "database volume, keys and data dir are kept" \
  'docker volume inspect "$BRAND-pgdata" >/dev/null && [ -s "/etc/$BRAND/db-password" ] && [ -s "/var/lib/$BRAND/data/encryption_key" ]'
check "a re-run restores it without asking anything" "$INSTALL </dev/null"
check "/readyz 200 after the restore" "$READY"
check "same admin and password after the restore" "$LOGIN"
same "restore keeps every secret" "$before" "$(box "$SECRETS")"

echo "==> Purge"
check "purge refuses without a terminal" "! $INSTALL --uninstall --purge </dev/null && [ -d /etc/\$BRAND ]"
check "purge refuses a wrong confirmation" "printf 'nope\n' | script -qec '$INSTALL --uninstall --purge' /dev/null; [ -d /etc/\$BRAND ] && docker volume inspect \"\$BRAND-pgdata\" >/dev/null"
check "purge after typing the name removes everything it created" \
  "printf '%s\n' \"\$BRAND\" | script -qec '$INSTALL --uninstall --purge' /dev/null && ! docker volume inspect \"\$BRAND-pgdata\" >/dev/null 2>&1 && [ ! -e /etc/\$BRAND ] && [ ! -e /var/lib/\$BRAND ] && ! id \"\$BRAND\" >/dev/null 2>&1"

echo
echo "$passed passed, $failed failed"
((failed == 0))
