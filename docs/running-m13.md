# Running M13: production self-hosting

M13 packages the Control Server and Relay as unprivileged multi-stage container
images and provides a Compose stack with PostgreSQL and Caddy. PostgreSQL is
reachable only on an internal Docker network. The Control API has no published
container port and is exposed only through Caddy HTTPS. The Relay remains a
separate QUIC/UDP service because an HTTP reverse proxy cannot carry its framed
RemoteX data path.

## Prerequisites and DNS

Use a current Docker Engine with the Compose plugin and two DNS names pointing
to the host:

- `control.example.com` for the HTTPS Control API;
- `relay.example.com` for Relay QUIC.

Open TCP 80 and TCP/UDP 443 for Caddy, and UDP 7443 (or the configured Relay
port). Do not expose TCP 5432. Obtain a normal CA-issued certificate for the
Relay name and save its chain and key as described below. Caddy obtains and
renews the Control certificate independently.

## Create configuration and secrets

From the repository root:

```bash
cd deploy
cp .env.example .env
mkdir -p secrets
umask 077
openssl rand -hex 32 > secrets/postgres-password
password="$(cat secrets/postgres-password)"
printf 'postgres://remotex:%s@postgres:5432/remotex\n' "$password" > secrets/database-url
openssl rand -hex 32 > secrets/control-master-key
cp /secure/acme/relay-fullchain.pem secrets/relay-cert.pem
cp /secure/acme/relay-key.pem secrets/relay-key.pem
chmod 600 .env secrets/*
```

If you choose a database password containing URL-special characters,
percent-encode it in `database-url`. Edit `.env` with the real domains and ACME
email. Compose mounts runtime secrets read-only;
the secret directory is ignored by Git. Never paste secret values into the
Compose file, logs, issues, or screenshots.

## Validate and start

```bash
docker compose config --quiet
docker compose build --pull
docker compose up -d
docker compose ps
curl --fail https://control.example.com/health
curl --fail https://control.example.com/ready
```

`/health` reports process liveness. `/ready` also executes a PostgreSQL query and
returns HTTP 503 when persistence is unavailable. Relay exposes an internal
health endpoint on port 8081 for its container probe; only UDP 7443 is published.
Control and Relay emit newline-delimited JSON through `tracing`, suitable for a
container log collector. Identifiers and outcomes may be logged; credentials,
keys, clipboard content, terminal bytes, frames, and file payloads are not.

## Operations

Inspect only metadata-safe logs and service state:

```bash
docker compose ps
docker compose logs --since=15m control relay caddy
docker compose exec postgres pg_isready -U remotex -d remotex
```

Create an encrypted, access-controlled database backup:

```bash
docker compose exec -T postgres pg_dump -U remotex -d remotex -Fc > remotex.dump
```

Test restoration in a separate environment. To upgrade, back up PostgreSQL,
checkout the intended signed/tagged RemoteX revision, run `docker compose build
--pull`, then `docker compose up -d`. Migrations run before Control becomes ready.
Rollback application containers only when the database migration notes for that
version explicitly permit it.

Rotate the Relay certificate by replacing both PEM files atomically and
recreating Relay. Rotating `control-master-key` invalidates still-wrapped pending
Session material, so first let short-lived Sessions expire, replace the file,
and recreate Control. Database-password rotation requires updating PostgreSQL
and both password/URL secret files in one maintenance window.

## Production boundary

- Run Docker on a dedicated, patched host and restrict SSH/administrator access.
- Back up the database and Caddy data volume; protect backups like credentials.
- Put external rate limiting or a trusted network policy in front of the Control
  hostname when exposed to the public Internet.
- Keep Relay message/session/connection limits at their bounded defaults unless
  capacity testing justifies a change.
- Agents still require foreground approval by default. Deployment never enables
  unattended access, remote input, clipboard, files, or terminal by itself.
