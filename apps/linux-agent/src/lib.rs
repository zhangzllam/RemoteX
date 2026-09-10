//! Authorized Linux PTY and system-information capabilities.

use anyhow::Context;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use remotex_protocol::{
    MAX_TERMINAL_DATA_SIZE, SystemDisk, SystemGpu, SystemNetworkInterface, SystemSnapshot,
    TerminalId, TerminalMessage,
};
use std::{
    io::{Read, Write},
    process::Command,
};
use sysinfo::{Disks, Networks, System};
use tokio::sync::mpsc;

pub const MAX_TERMINALS_PER_SESSION: usize = 4;
pub const TERMINAL_OUTPUT_QUEUE: usize = 32;

pub struct TerminalSession {
    id: TerminalId,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl TerminalSession {
    pub fn open(
        id: TerminalId,
        columns: u16,
        rows: u16,
        output: mpsc::Sender<TerminalMessage>,
    ) -> anyhow::Result<Self> {
        validate_size(columns, rows)?;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open Linux PTY")?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        if !shell.starts_with('/') || shell.contains('\0') {
            anyhow::bail!("SHELL must be an absolute executable path");
        }
        let mut command = CommandBuilder::new(shell);
        command.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(command)
            .context("spawn authorized shell in PTY")?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let writer = pair.master.take_writer().context("take PTY writer")?;
        std::thread::Builder::new()
            .name(format!("remotex-pty-{id}"))
            .spawn(move || read_pty_output(id, &mut reader, &output))
            .context("spawn PTY reader")?;
        Ok(Self {
            id,
            master: pair.master,
            writer,
            child,
        })
    }

    pub fn input(&mut self, data: &[u8]) -> anyhow::Result<()> {
        if data.is_empty() || data.len() > MAX_TERMINAL_DATA_SIZE {
            anyhow::bail!("terminal input must contain 1 to {MAX_TERMINAL_DATA_SIZE} bytes");
        }
        self.writer.write_all(data).context("write PTY input")?;
        self.writer.flush().context("flush PTY input")
    }

    pub fn resize(&self, columns: u16, rows: u16) -> anyhow::Result<()> {
        validate_size(columns, rows)?;
        self.master
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize PTY")
    }

    pub fn close(mut self) -> anyhow::Result<Option<u32>> {
        self.child.kill().context("terminate PTY child")?;
        Ok(self.child.wait().ok().map(|status| status.exit_code()))
    }

    #[must_use]
    pub const fn id(&self) -> TerminalId {
        self.id
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _result = self.child.kill();
    }
}

fn read_pty_output(id: TerminalId, reader: &mut dyn Read, output: &mpsc::Sender<TerminalMessage>) {
    let mut buffer = vec![0_u8; MAX_TERMINAL_DATA_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if output
                    .blocking_send(TerminalMessage::Output {
                        terminal_id: id,
                        data: buffer[..count].to_vec(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _result = output.blocking_send(TerminalMessage::Closed {
        terminal_id: id,
        exit_code: None,
    });
}

fn validate_size(columns: u16, rows: u16) -> anyhow::Result<()> {
    if !(2..=1_000).contains(&columns) || !(2..=1_000).contains(&rows) {
        anyhow::bail!("terminal size must be between 2 and 1000 cells");
    }
    Ok(())
}

#[must_use]
pub fn system_snapshot() -> SystemSnapshot {
    let mut system = System::new_all();
    system.refresh_all();
    let disks = Disks::new_with_refreshed_list()
        .list()
        .iter()
        .take(128)
        .map(|disk| SystemDisk {
            name: disk.name().to_string_lossy().into_owned(),
            mount_point: disk.mount_point().to_string_lossy().into_owned(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
        })
        .collect();
    let networks = Networks::new_with_refreshed_list();
    let network_interfaces = networks
        .iter()
        .take(128)
        .map(|(name, data)| SystemNetworkInterface {
            name: name.clone(),
            received_bytes: data.total_received(),
            transmitted_bytes: data.total_transmitted(),
        })
        .collect();
    SystemSnapshot {
        hostname: System::host_name().unwrap_or_else(|| "unknown".to_owned()),
        operating_system: format!(
            "{} {}",
            System::name().unwrap_or_else(|| "Linux".to_owned()),
            System::os_version().unwrap_or_default()
        )
        .trim()
        .to_owned(),
        kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".to_owned()),
        cpu_model: system
            .cpus()
            .first()
            .map_or_else(|| "unknown".to_owned(), |cpu| cpu.brand().to_owned()),
        cpu_count: u32::try_from(system.cpus().len()).unwrap_or(u32::MAX),
        total_memory_bytes: system.total_memory(),
        used_memory_bytes: system.used_memory(),
        uptime_seconds: System::uptime(),
        disks,
        network_interfaces,
        gpus: nvidia_gpus(),
    }
}

fn nvidia_gpus() -> Vec<SystemGpu> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .take(16)
        .filter_map(parse_gpu_line)
        .collect()
}

fn parse_gpu_line(line: &str) -> Option<SystemGpu> {
    let values = line.split(',').map(str::trim).collect::<Vec<_>>();
    if values.len() != 5 || values[0].is_empty() {
        return None;
    }
    Some(SystemGpu {
        name: values[0].chars().take(128).collect(),
        utilization_percent: values[1].parse().ok(),
        memory_used_bytes: values[2]
            .parse::<u64>()
            .ok()
            .map(|value| value.saturating_mul(1024 * 1024)),
        memory_total_bytes: values[3]
            .parse::<u64>()
            .ok()
            .map(|value| value.saturating_mul(1024 * 1024)),
        temperature_celsius: values[4].parse().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_sizes_and_payloads_are_bounded() {
        assert!(validate_size(80, 24).is_ok());
        assert!(validate_size(0, 24).is_err());
        assert!(validate_size(80, 1_001).is_err());
    }

    #[test]
    fn gpu_csv_parser_is_optional_and_bounded() {
        let gpu = parse_gpu_line("NVIDIA A10, 42, 1024, 24576, 55").expect("GPU");
        assert_eq!(gpu.name, "NVIDIA A10");
        assert_eq!(gpu.utilization_percent, Some(42));
        assert!(parse_gpu_line("malformed").is_none());
    }

    #[test]
    fn system_snapshot_has_finite_bounded_collections() {
        let snapshot = system_snapshot();
        assert!(!snapshot.hostname.is_empty());
        assert!(snapshot.disks.len() <= 128);
        assert!(snapshot.network_interfaces.len() <= 128);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pty_runs_utf8_input_resizes_and_closes() {
        let (sender, mut receiver) = mpsc::channel(TERMINAL_OUTPUT_QUEUE);
        let id = TerminalId::new();
        let mut session = TerminalSession::open(id, 80, 24, sender).expect("open PTY");
        session.resize(100, 30).expect("resize PTY");
        session
            .input("printf 'remotex-终端-ok\\n'\n".as_bytes())
            .expect("send UTF-8 command");

        let output = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            while let Some(message) = receiver.recv().await {
                if let TerminalMessage::Output { data, .. } = message {
                    bytes.extend(data);
                    if String::from_utf8_lossy(&bytes).contains("remotex-终端-ok") {
                        break;
                    }
                }
            }
            bytes
        })
        .await
        .expect("PTY output timeout");
        assert!(String::from_utf8_lossy(&output).contains("remotex-终端-ok"));
        session.close().expect("close PTY");
    }
}
