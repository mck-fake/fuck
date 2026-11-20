---
name: fuck-install
description: >
  Install the platform (the Temps fork, branded "fuck") as a production or a
  development environment. One idempotent installer asks for the environment,
  admin email, password, first and last name and (production) the domain,
  falling back to <public-ip>.sslip.io. Production installs the
  checksum-verified release, public with TLS. Development builds this checkout
  in Docker, on loopback with debug logs, and `--upgrade` rebuilds your
  changes. Both get TimescaleDB and a systemd unit with no secret in it,
  upgrade with automatic rollback, uninstall, and an end-to-end sandbox test.
  For hot-reload hacking it hands off to start-temps. Invoke for "install
  fuck", "install temps", "set up a server", "production install", "dev
  environment", "validate the installer".
---

# Install the platform (dev and production)

The product is branded **fuck**: install paths, system user, service alias,
CLI command and messages use that name. It is one variable (`BRAND` in
`scripts/install.sh`). The binary, its `TEMPS_*` environment variables and the
`temps.service` unit file keep the upstream name, because the binary looks for
them.

| You want to | Run |
|---|---|
| Run a public server on the verified release | `sudo scripts/install.sh` and answer **production** |
| Run a public server on your fork's code before it has a release | `sudo scripts/install.sh --source checkout` (builds this checkout on the server) |
| Run your fork's code on your own machine, isolated from anything else there | `scripts/devbox.sh up` (Docker only: no sudo, Rust or Bun) |
| Run your fork's code as a local service on a dedicated host | `sudo scripts/install.sh --env development` |
| Hot-reload while editing Rust or the console | `scripts/dev-check.sh`, then the [start-temps](../start-temps/SKILL.md) skill |
| Prove the installer works before touching a server | `scripts/test-prod-sandbox.sh` (add `--env development` for the dev build) |
| Install upstream Temps on a remote host over SSH | the [temps-platform-setup](../../../skills/temps-platform-setup/SKILL.md) skill (official `deploy.sh`) |

Paths below are relative to this skill directory.

## Environments

The first question is the environment. It is remembered in `install.conf`; a
host switches environment only through `--uninstall` and a new install.

| | `production` (default) | `development` |
|---|---|---|
| Binary | The fork's release from GitHub (`mck-fake/fuck`), SHA-256 verified (`beta` unless `--version`/`--channel`); `--source checkout` builds this checkout instead | Built from this checkout with the repository `Dockerfile` (`artifacts-source`, `fast` profile) |
| Network | Public `:80/:443`, TLS on demand, your domain or `<ip>.sslip.io` (`--mode local` for loopback) | Loopback only, `127-0-0-1.sslip.io`, no domain question |
| Logs | `info` | `debug` |
| `--upgrade` | Next release, with DB backup and automatic rollback | Rebuilds your changes (BuildKit cache) with the same backup and rollback |
| Needs | 2 GB RAM, 10 GB disk | About 8 GB RAM for the build, `docker-buildx` (installed if missing) |

The first development build compiles the whole workspace and the console and
takes a long time; later rebuilds reuse the cargo cache. On Windows, run it
inside WSL2 from a clone under `~/`: a checkout on `/mnt/c` builds far slower.

## Devbox: development on your own machine

`scripts/devbox.sh` runs `install.sh --env development` inside a persistent
systemd container with its own Docker daemon (`fuck-devbox`). Nothing it runs
shares a port, unit file, container or network with the host. That matters:
the platform uses fixed Docker names (`temps-preview-gateway`,
`temps-app-network`), so a second instance on the same Docker daemon would
fight an existing one over them.

```bash
scripts/devbox.sh up        # first time: asks the questions, builds the checkout, installs
scripts/devbox.sh dev       # after editing Rust: syncs changed files, incremental debug build, restarts (fast)
scripts/devbox.sh rebuild   # production-like: copies the checkout in, rebuilds, restarts
scripts/devbox.sh test -p temps-auth --lib   # cargo test in the repo toolchain, inside the box
scripts/devbox.sh status    # service state and URLs
scripts/devbox.sh logs      # follow the platform's debug logs
scripts/devbox.sh shell     # root shell inside the box
scripts/devbox.sh down      # stop it; data and build cache stay
scripts/devbox.sh destroy   # delete it with its data (asks you to type its name)
```

- **Console:** `http://localhost:9081`. **Apps:** `http://<name>-production.127-0-0-1.sslip.io:9180`.
  Both are published only on this machine's loopback.
- **Code:** it stays where you edit it. Every `up` and `rebuild` copies the
  checkout into the box as LF, so a Windows checkout works unchanged.
- **Windows:** run the commands through WSL, where Docker lives, for example
  `wsl -- bash /mnt/c/<path to checkout>/.agents/skills/fuck-install/scripts/devbox.sh rebuild`.

How a change reaches the running platform:

| You changed | How to see it |
|---|---|
| The console (`web/`) | Run the dev server against the devbox. Saving a file reloads the page in about half a second; no rebuild. |
| Rust (`crates/`) | `devbox.sh dev`. Only files whose content changed are synced, so cargo recompiles just the crates you touched (debug, incremental), the embedded console is rebuilt only when `web/` changed, and the binary is swapped and restarted. The first run is a full build; later ones take about a minute or two. No backup or rollback. |
| Production-like check | `devbox.sh rebuild`: the release-style Docker build with backup and automatic rollback, as `--upgrade` does on a server. Slow (around ten minutes). |
| Rust tests | `devbox.sh test <cargo test args>`. Runs in the Dockerfile's `toolchain` stage (plus static OpenSSL for test-only dependencies) on the box's own Docker, so `TestDatabase` tests get a real Postgres there; each run copies the checkout to its own directory, and the `target` volume makes reruns incremental. |

```bash
cd web
TEMPS_API_TARGET=http://127.0.0.1:9081 TEMPS_CONSOLE_TARGET=http://127.0.0.1:9081 bun dev
# PowerShell:
#   $env:TEMPS_API_TARGET="http://127.0.0.1:9081"; $env:TEMPS_CONSOLE_TARGET=$env:TEMPS_API_TARGET; bun dev
```

Open the address it prints (`http://localhost:3000` when that port is free).
Use `127.0.0.1`, not `localhost`: Node resolves `localhost` to IPv6 first, and
the devbox publishes IPv4 only. Point both variables at the console listener
(`:9081`): the devbox's public proxy (`:9180`) answers `/api` with a redirect
that the dev proxy rejects.

The box is privileged (it runs its own Docker daemon). Its database, keys and
build cache live in the `fuck-devbox-*` volumes.

## Hot-reload development (start-temps)

1. Run `scripts/dev-check.sh`. It is read-only and prints the exact fix for
   every gap: Rust toolchain (pinned in `rust-toolchain.toml`), the build
   packages from CONTRIBUTING.md, `wasm-pack`, Bun, `web/node_modules`, Docker
   and the TimescaleDB container.
2. Follow [start-temps](../start-temps/SKILL.md). Each checkout gets its own
   port slot and `temps_s<N>` database, so worktrees never collide.

On Windows, work inside WSL2: pingora does not build natively. The check
flags a checkout on `/mnt/c` (slow builds) and a `bun` that resolves to the
Windows binary. If an installed platform already runs on the machine, it owns
slot 0's ports and the `temps` database. Never point a checkout at that database.

## Install

```bash
git clone --depth 1 <this repository> fuck && cd fuck
sudo .agents/skills/fuck-install/scripts/install.sh --check   # read-only
sudo .agents/skills/fuck-install/scripts/install.sh
```

On the first run it asks, in this order:

0. **Environment**: `production` (Enter) or `development`.
1. **Admin email**: the login, and the Let's Encrypt contact.
2. **First name** and **last name**: stored as the admin's display name.
3. **Admin password**: asked twice and hidden. It must have 8-128 characters
   with an uppercase letter, a lowercase letter, a digit and a symbol (the
   platform's own policy). It goes to setup through the environment and is
   never written to disk or to the log.
4. **Domain** (production): press Enter to take `<public-ip>.sslip.io`. That
   is a `.io` wildcard DNS name that resolves to the server with no
   registration or DNS records. With your own domain, point `example.com` and
   `*.example.com` at the server.

`--mode local` binds everything to loopback and uses `127-0-0-1.sslip.io`
without asking. A re-run asks nothing and keeps every secret. It refuses a
different `--mode` or `--domain` than the first install chose: the domain and
TLS were set for that mode.

Releases come from the fork's repository (`release_repo` in
`scripts/install.sh`, overridable with `TEMPS_RELEASE_REPO=owner/name`). The
same repository feeds `temps upgrade` and the console's self-update
(`GITHUB_RELEASES_API` in `crates/temps-cli/src/commands/upgrade.rs`), so an
update never replaces the fork with upstream Temps. Until the fork publishes
its first release, use `--source checkout`.

| Flag (for unattended runs) | Effect |
|---|---|
| `--env production` / `--env development` | Answer the environment question |
| `--email`, `--first-name`, `--last-name` | Answer the questions |
| `--password-file PATH` | Read the password from a root-only file (never pass it as a flag value) |
| `--domain example.com` | Your domain; without it, the sslip.io fallback |
| `--mode public` (default) / `--mode local` | Public `:80/:443` with on-demand TLS, or loopback only |
| `--version vX.Y.Z` / `--channel stable\|beta\|nightly` | Production only: version for the first install (defaults to `beta`, like `deploy.sh`) or for `--upgrade` |
| `--source release` (default) / `--source checkout` | Production only: the fork's published release, or this checkout built in Docker on the server (about 8 GB RAM, slow first build). Remembered, so `--upgrade` rebuilds; `--upgrade --source ...` switches it |
| `--check` | Preflight only; changes nothing |
| `--upgrade` | Back up the database, install the new binary, roll back on its own if it is not healthy within 120s |
| `--uninstall` | Remove the service, binary, CLI and database container; keep the data |
| `--uninstall --purge` | Also delete the database, keys and data, after typing the name in a terminal |

Without a terminal, a missing or invalid answer stops the run before anything
changes.

What it does, in order. Every step checks before it acts, and nothing changes
until the preflight passes and the answers are valid:

1. **Preflight**: Linux, root, systemd, amd64/arm64, glibc >= 2.35, RAM, disk,
   free ports (80, 443, 8081, 16432), and no platform unit or container that it
   did not create. A `deploy.sh` install is refused, never adopted.
2. **Questions**: the four answers above.
3. **Packages**: the distro's signed `docker.io` and `docker-compose-v2` if
   Docker is missing, plus `nftables`. Log rotation goes into `daemon.json`
   only on a daemon this run installed.
4. **Database**: `fuck-postgres` (TimescaleDB) on `127.0.0.1:16432`, with
   volume `fuck-pgdata` and `--restart unless-stopped`.
5. **Binary**: `scripts/install.sh` (SHA-256 verified) into `/opt/fuck`. The
   `fuck` command (and `temps`, unless another program owns that name) runs it
   with this server's database URL and data dir, as the service user, so
   `sudo fuck reset-admin-password`, `doctor` and `migrate` work as they are.
6. **Setup**: admin account (email, name, password), encryption keys,
   migrations and the `*.<domain>` wildcard, run as the `fuck` user.
7. **Service**: renders `templates/temps.service`, which declares the
   `fuck.service` alias. The unit runs as the unprivileged `fuck` user with
   only `CAP_NET_BIND_SERVICE` and `CAP_NET_ADMIN`, and keeps the console/admin
   API on `127.0.0.1:8081`. It then waits for `/readyz`.

| Path | Holds |
|---|---|
| `/etc/fuck/` (0700) | `temps.env`, `db.env`, `db-password`, `install.conf`: all 0600 |
| `/var/lib/fuck/data` | Data dir (`encryption_key`, `auth_secret`, GeoLite2, logs) |
| `/var/log/fuck-install.log` | Install transcript (0600; never contains the admin password) |

Operate it with `systemctl status fuck`, `journalctl -u temps -f` and
`sudo fuck --help`.

### Upgrade and uninstall

`--upgrade` takes a `pg_dump` into `/var/lib/fuck/backups` (root-only, newest
three kept), then downloads the new release into a staging dir. A failed
download leaves the running binary untouched. It swaps the files in with a
rename and restarts. If `/readyz` does not answer within 120s, it puts the
previous binary back and exits non-zero. It never restores the database on its
own; the error names the backup and the `pg_restore` command. Refresh the unit
after pulling a newer installer by re-running it without flags.

`--uninstall` removes only what the installer created. It leaves app and
service containers deployed through the platform alone, and says how many
remain. Re-running the installer afterwards brings the same instance back with
the same admin, password and data.

### Why not `deploy.sh`

It lives outside this repository, so the fork cannot version or change it. It
also has three security gaps:

- It writes `--database-url` (with the password) into the world-readable
  systemd unit.
- It binds the console/admin API to `0.0.0.0:8081`.
- It runs remote `get.docker.com`, `get.acme.sh` and nodesource scripts.

### Rules for agents

- Install only on a machine the user named. Run `--check` first and report it.
- Ask the user to type the password in their own terminal or to provide a
  root-only file. Never put it in a command line, a chat message or a log.
- Never run it where the platform is already installed by other means, and
  never stop or remove a service or container to free a port. Report the owner
  and ask.
- Confirm with the user before `--upgrade` or `--uninstall`. Never run
  `--purge` for them: it is theirs to type.

### Not included on purpose

- **ClickHouse**: the platform falls back to PG-only mode. Add it on servers
  with 8 GB or more when you want ClickHouse-backed analytics.
- **Firewall changes**: the installer only prints which ports to open.
- **Edits to an existing Docker daemon's config.**

## Validate before touching a server

```bash
.agents/skills/fuck-install/scripts/test-prod-sandbox.sh          # add --keep to inspect afterwards
```

It boots a privileged Ubuntu 24.04 systemd container with its own Docker
daemon. The production scenario installs and upgrades between two published
releases; they come from upstream (`TEMPS_RELEASE_REPO=gotempsh/temps`) until the
fork publishes its own. It answers the questions through a real terminal, then asserts:

- the input gates: invalid email, weak password, no password without a
  terminal, a foreign `temps.service`, and the sslip.io domain fallback;
- that the answers took effect: the admin name in the database, and a login
  with the chosen password;
- health, through the `fuck` alias and CLI as well, and admin commands that
  find the database;
- the security properties: loopback binds, 0600 secrets, a password that never
  reaches disk, the log or any process argv, and a non-root service user;
- a flag-less re-run that asks nothing and keeps every secret;
- a restart;
- a real upgrade from an older beta, with its database backup;
- the automatic rollback from a release that cannot start;
- uninstall, then a re-run that restores the same instance;
- purge, which refuses without a terminal or with the wrong confirmation.

`--env development` instead ships the whole checkout into the sandbox and
asserts:

- the binary was built from the checkout, not downloaded, with debug logs and
  loopback binds;
- the bundled GeoLite2 databases, login and admin commands;
- that switching the host to production is refused;
- a rebuild through `--upgrade` that keeps every secret.

That run compiles the whole workspace and takes much longer.

The first run downloads about 4 GB. It touches nothing on the host except its
own container, volumes and the `temps-install-sandbox` image.
