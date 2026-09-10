# Personal IP deployment

The built-in deployment uses `https://39.96.68.170:7443` for Control and
`39.96.68.170:7443` over UDP for Relay. Computers can be on different networks;
they make outbound connections to the server. Each controlled computer still
requires Remote Access to be explicitly enabled and sessions authorized.
No SSH credentials are installed on client computers.

## Server entry point

`compose.personal.yml` is an additive overlay on `compose.yml`:

```sh
docker compose -f compose.yml -f compose.personal.yml up -d --no-deps control-ip
```

The Caddy service proxies the existing Control API, including WebSocket upgrade
requests, without replacing Control, Relay, PostgreSQL, or the original Caddy.
This does not convert RemoteX into an arbitrary `ws://` client: its Control REST
API and QUIC Relay protocol remain unchanged.

Allow **both TCP 7443 and UDP 7443** inbound in the cloud firewall/security group
and any host firewall. TCP serves Control; UDP serves Relay. Keep SSH restricted
to the administrator's trusted source addresses. No change to a client's router
or inbound port forwarding is normally needed; client networks must allow the
required outbound traffic. VPN TUN routing or network policy can still block it.

## Trust and migration

The IP endpoint has a Caddy internal-CA certificate with an IP subject alternative
name. `crates/control-client/resources/personal-control-root.pem` contains only
the public root certificate retrieved from the deployment over trusted SSH.
Desktop, Windows Agent, and Linux Agent use the same shared Control client.
For this exact endpoint, it trusts only that root, disables HTTP redirects, and
does not use environment/system HTTP proxies. It does not bypass TLS checks or
install a root into the operating system. Custom server URLs retain their normal
trust/proxy policy. This cannot override a VPN's packet-level routing.

The exact former default `https://control.39-96-68-170.sslip.io` is migrated
automatically. Unrelated custom hosts, paths, and ports are not migrated.
Relay's known hostname is resolved to its deployed IP without DNS, but its TLS
server name and CA validation remain in use. Relay certificate renewal still
depends on the original server's certificate automation.

Back up the server's `personal_caddy_data` volume securely: it holds CA private
keys. Never distribute that volume or its keys. Caddy renews its leaf certificate
automatically; losing/replacing this CA requires a deliberate client trust update.
The public root bundled on 2026-09-08 expires in July 2036. Other deployments
must issue their own certificates and must not reuse this deployment's identity.

## Verification

```powershell
cargo test --locked -p remotex-control-client
cargo test --locked -p remotex-control-client deployed_control -- --ignored
cargo test --locked -p remotex-desktop deployed_managed_server -- --ignored
```

The live checks are opt-in and require access to the deployed server. Readiness
and TLS checks alone do not demonstrate a full screen-control session: verify
two computers on different networks, explicit approval, image/input, reconnect,
and VPN on/off before declaring cross-network acceptance complete.

This configuration is not a guarantee against provider filtering or a statement
of regulatory exemption. If an open port remains unreachable, inspect cloud
rules, host firewall, packet capture, and provider restrictions before changing
application authentication or weakening encryption.

### Local verification on 2026-09-08

- Windows Agent, Desktop, and shared client: 23 unit tests passed; targeted
  Clippy checks passed with warnings denied.
- Public Relay QUIC/TLS check passed against UDP 7443.
- New Control endpoint: initially verified the IP certificate and `/ready`
  response from inside the server. After the user added the cloud ingress rule,
  the shared client's public Control readiness test and Desktop's combined
  Control/Relay check both passed. Full two-computer acceptance remains pending.
- Windows release Agent, TypeScript/Vite, Desktop, and NSIS build passed. This
  local installer was not published or automatically installed.
- Linux cross-check could not complete on Windows because the
  `x86_64-linux-gnu-gcc` cross-compiler is unavailable. Linux runtime validation
  is still required; the Windows installer does not contain the Linux Agent.
- The installed Windows application and saved server configuration were found
  updated to the IP endpoint. An attempted isolated Agent runtime probe was
  blocked by execution policy and was not run; no successful device heartbeat
  or authorized screen-control session is inferred from the readiness checks.
