# Runtime secrets

Create these files locally with mode `0600`; their values are never committed:

- `postgres-password`: a random PostgreSQL password;
- `database-url`: `postgres://remotex:PASSWORD@postgres:5432/remotex` using the
  same password (percent-encode URL-special characters);
- `control-master-key`: exactly 64 hexadecimal characters;
- `relay-cert.pem`: a public CA-issued certificate chain for the Relay hostname;
- `relay-key.pem`: its matching private key.

The Compose stack mounts them read-only under `/run/secrets`. Do not use a
self-signed Relay certificate for clients outside a development environment.
