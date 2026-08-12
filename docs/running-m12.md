# Running M12: Linux server Agent

M12 adds a Linux Agent for explicitly authorized terminal, file, and read-only
system-information access. It does not capture a Linux desktop or accept remote
input. Terminal and system capabilities are independent Session permissions and
the Agent enforces them after authenticated end-to-end decryption.

## Interactive operation

Build and run on a Linux host with stable Rust:

```bash
cargo build --release -p remotex-linux-agent
export REMOTEX_CONTROL_URL=https://control.example.com
export REMOTEX_RELAY_ADDRESS=relay.example.com:7443
export REMOTEX_RELAY_SERVER_NAME=relay.example.com
export REMOTEX_CA_CERT=/etc/remotex/ca.pem
export REMOTEX_FILE_ROOTS='Data=/srv/remotex'
./target/release/remotex-linux-agent
```

The Agent prints its nine-digit Device ID. By default every connection request
must be accepted in the foreground console. The accepted Session and granted
terminal/files/system permissions remain visible until disconnect. `Ctrl+C`
ends the active Session and cleans up all PTY children and transfer handles.

## Optional systemd operation

Create an unprivileged `remotex` account, install the binary and the files from
`packaging/linux`, then review the environment before enabling anything:

```bash
sudo install -o root -g root -m 0755 target/release/remotex-linux-agent /usr/local/bin/
sudo install -o root -g root -m 0644 packaging/linux/remotex-agent.service /etc/systemd/system/
sudo install -o root -g root -m 0600 packaging/linux/agent.env.example /etc/remotex/agent.env
sudo systemctl daemon-reload
sudo systemctl enable --now remotex-agent
```

Because a system service cannot show an interactive prompt, it rejects incoming
Sessions unless `REMOTEX_UNATTENDED=true` and a strong
`REMOTEX_UNATTENDED_SECRET` are both explicitly configured. Keep the environment
file mode `0600`; never put the secret in source control. The service is not
installed or enabled automatically.

## Limits and behavior

- At most four PTYs may exist in one Session; each input/output message is
  bounded and terminal output uses a bounded queue.
- PTYs inherit only the Agent service account's OS privileges. Run the Agent as
  a dedicated non-root user and restrict `REMOTEX_FILE_ROOTS`.
- Resize, UTF-8 input, basic ANSI SGR colors, `Ctrl+C`, and explicit close are
  supported. Disconnect kills remaining PTY children.
- CPU, memory, disk, network, uptime, and optional NVIDIA data are read-only and
  collection sizes are bounded. GPU data is omitted when `nvidia-smi` is absent.
- The Linux Agent reuses M7 virtual-root validation, checksums, resume, cancel,
  and queue backpressure rather than exposing arbitrary filesystem paths.
