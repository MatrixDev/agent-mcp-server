mod async_port;
mod orchestrator;

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::bail;
use mcp_qemu_agent_proto::{AgentPacket, GuestResponse, HostRequest};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinSet;
use tracing::{debug, error, info, instrument, warn};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::async_port::AsyncPort;
use crate::orchestrator::Orchestrator;

////////////////////////////////////////////////////////////////////////////////
const CHANNEL_CAPACITY: usize = 256;
const PORTS_DIR: &str = "/dev/virtio-ports";
const AGENT_PORT: &str = "rdk.agent0";
const PORT_ATTEMPTS: usize = 10;
const RETRY_DELAY: Duration = Duration::from_secs(1);

////////////////////////////////////////////////////////////////////////////////
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .init();

    let port_path = resolve_port_path().await?;
    info!("serving on {}", port_path.display());

    loop {
        match AsyncPort::open(&port_path) {
            Ok(e) => run_session(e).await,
            Err(e) => error!("failed to open {}: {e}", port_path.display()),
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn run_session(port: AsyncPort) {
    let (request_tx, request_rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
    let (response_tx, response_rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);

    let mut join_set = JoinSet::new();
    join_set.spawn(port_reader_worker(port.clone(), request_tx));
    join_set.spawn(port_writer_worker(port, response_rx));
    join_set.spawn(Orchestrator::run(request_rx, response_tx));
    join_set.join_all().await;
}

////////////////////////////////////////////////////////////////////////////////
async fn resolve_port_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::args_os().nth(1) {
        return Ok(path.into());
    }

    let named = Path::new(PORTS_DIR).join(AGENT_PORT);
    for _ in 0..PORT_ATTEMPTS {
        // WARNING: a char device, not a regular file. `metadata` follows the symlink udev
        // leaves here, and nothing under /dev is a file, so testing for one finds nothing.
        if tokio::fs::metadata(&named)
            .await
            .is_ok_and(|e| e.file_type().is_char_device())
        {
            return Ok(named);
        }

        if let Some(path) = find_first_entry(PORTS_DIR, |_| true).await {
            warn!("{AGENT_PORT} is missing, falling back to {}", path.display());
            return Ok(path);
        }

        if let Some(path) = find_first_entry("/dev", |file_name| {
            file_name.strip_prefix("vport").is_some_and(|e| e.contains('p'))
        })
        .await
        {
            warn!("{PORTS_DIR} is empty, falling back to {}", path.display());
            return Ok(path);
        }

        debug!("no virtio-serial port yet, retrying");
        tokio::time::sleep(RETRY_DELAY).await;
    }

    bail!("no virtio-serial port under {PORTS_DIR} after {PORT_ATTEMPTS} attempts")
}

////////////////////////////////////////////////////////////////////////////////
async fn find_first_entry(dir: &str, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let mut paths = Vec::new();
    let mut dir = tokio::fs::read_dir(dir).await.ok()?;

    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();

        if let Some(file_name) = path.file_name()
            && let Some(file_name) = file_name.to_str()
            && matches(file_name)
        {
            paths.push(path);
        }
    }

    paths.sort();
    paths.into_iter().next()
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, name = "port_reader")]
async fn port_reader_worker(reader: AsyncPort, tx: tokio::sync::mpsc::Sender<AgentPacket<HostRequest>>) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::with_capacity(4096);

    loop {
        buffer.clear();

        tokio::select! {
            e = reader.read_until(0, &mut buffer) => match e {
                Ok(0) => return info!("the host closed the port"),
                Ok(_) => {}
                Err(e) => return error!("failed to read from the port: {e:?}"),
            },
            _ = tx.closed() => return info!("reader channel closed"),
        }

        match mcp_qemu_agent_proto::deserialize_cobs(&mut buffer) {
            Ok(e) => {
                debug!("recv: {e:?}");

                if tx.send(e).await.is_err() {
                    return info!("reader channel closed");
                }
            }
            Err(e) => warn!("failed to decode a {} byte frame: {e:?}", buffer.len()),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, name = "port_writer")]
async fn port_writer_worker(mut writer: AsyncPort, mut rx: tokio::sync::mpsc::Receiver<AgentPacket<GuestResponse>>) {
    while let Some(response) = rx.recv().await {
        debug!("send: {response:?}");

        let packet = match mcp_qemu_agent_proto::serialize_cobs(&response) {
            Ok(e) => e,
            Err(e) => {
                warn!("failed to serialize a response: {e:?}");
                continue;
            }
        };

        if let Err(e) = writer.write_all(&packet).await {
            return error!("failed to write to the port: {e:?}");
        }

        if let Err(e) = writer.flush().await {
            return error!("failed to flush the port: {e:?}");
        }
    }

    debug!("writer channel closed");
}
