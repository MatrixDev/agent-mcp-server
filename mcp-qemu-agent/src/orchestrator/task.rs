use std::os::unix::process::ExitStatusExt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use mcp_qemu_agent_proto::{
    AgentPacket, CommandExec, CommandFinished, CommandId, CommandOutput, CommandStream, GuestResponse, RequestId,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::orchestrator::OrchestratorMessage;

////////////////////////////////////////////////////////////////////////////////
const MAX_LINE: usize = 64 * 1024;
const KILL_GRACE: Duration = Duration::from_secs(2);
const NO_GROUP: i32 = 0;

////////////////////////////////////////////////////////////////////////////////
pub struct Task {
    id: CommandId,
    process_group_id: Arc<AtomicI32>,
}

impl Task {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn spawn(
        request_id: RequestId,
        request: &CommandExec,
        tx: Sender<OrchestratorMessage>,
    ) -> anyhow::Result<Task> {
        let mut child = Command::new(&request.command)
            .args(&request.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to get stdout handle"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("failed to get stderr handle"))?;

        // `process_group(0)` makes the group id the child's own pid
        // `id` is still Some because nothing has reaped it yet
        let pid = child.id();
        let pid = pid.ok_or_else(|| anyhow!("the command exited before it was tracked"))?;
        let process_group_id = Arc::new(AtomicI32::new(pid as i32));

        let id = request.id;
        let pumps = [
            tokio::spawn(dump_stream(request_id, id, CommandStream::Stdout, stdout, tx.clone())),
            tokio::spawn(dump_stream(request_id, id, CommandStream::Stderr, stderr, tx.clone())),
        ];

        tokio::spawn(supervise(request_id, id, child, process_group_id.clone(), pumps, tx));

        Ok(Self { id, process_group_id })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn kill(&self) {
        let process_group_id = self.process_group_id.clone();

        tokio::spawn(async move {
            if !signal_group(&process_group_id, libc::SIGTERM) {
                return;
            }

            tokio::time::sleep(KILL_GRACE).await;

            if signal_group(&process_group_id, libc::SIGKILL) {
                warn!("a process group ignored SIGTERM, killed it");
            }
        });
    }
}

impl Drop for Task {
    ////////////////////////////////////////////////////////////////////////////////
    fn drop(&mut self) {
        debug!("{:?} abandoned, killing its process group", self.id);
        self.kill();
    }
}

////////////////////////////////////////////////////////////////////////////////
fn signal_group(process_group_id: &AtomicI32, signal: i32) -> bool {
    let process_group_id = process_group_id.load(Ordering::Relaxed);
    if process_group_id == NO_GROUP {
        return false;
    }

    // WARNING: negated, which is what makes this the whole group rather than one process
    if unsafe { libc::kill(-process_group_id, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        debug!("failed to signal process group {process_group_id}: {error}");
        return false;
    }

    true
}

////////////////////////////////////////////////////////////////////////////////
async fn supervise(
    request_id: RequestId,
    command_id: CommandId,
    mut child: Child,
    process_group_id: Arc<AtomicI32>,
    pumps: [JoinHandle<()>; 2],
    tx: Sender<OrchestratorMessage>,
) {
    // WARNING: the output is drained before the child is reaped, so every line of it is on its
    // way to the host before the CommandFinished that closes the task off
    for pump in pumps {
        let _ = pump.await;
    }

    let status = child.wait().await;

    // WARNING: cleared before the task can be dropped below, or the kill that comes with that
    // drop would go to whatever the system has since handed the pid to
    process_group_id.store(NO_GROUP, Ordering::Relaxed);

    let response = match status {
        Ok(status) => CommandFinished {
            id: command_id,
            code: status.code(),
            signal: status.signal(),
        },
        Err(e) => {
            warn!("failed to reap {command_id:?}: {e:?}");

            CommandFinished {
                id: command_id,
                code: None,
                signal: None,
            }
        }
    };

    debug!("{command_id:?} finished: {response:?}");

    let response = AgentPacket {
        id: request_id,
        content: GuestResponse::CommandFinished(response),
    };

    if tx.send(OrchestratorMessage::GuestResponse(response)).await.is_err() {
        return;
    }

    let _ = tx.send(OrchestratorMessage::TaskFinished(command_id)).await;
}

////////////////////////////////////////////////////////////////////////////////
async fn dump_stream(
    request_id: RequestId,
    command_id: CommandId,
    stream: CommandStream,
    source: impl AsyncRead + Unpin,
    tx: Sender<OrchestratorMessage>,
) {
    let reader = &mut BufReader::new(source);
    let buffer = &mut Vec::new();

    loop {
        buffer.clear();

        // WARNING: capped, so a command that never emits a newline cannot grow this without
        // bound. A longer line simply arrives as several.
        match reader.take(MAX_LINE as u64).read_until(b'\n', buffer).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => break warn!("failed to read {stream:?} of {command_id:?}: {e:?}"),
        }

        let response = CommandOutput {
            id: command_id,
            stream,
            line: trim_terminator(buffer).to_vec(),
        };

        let response = AgentPacket {
            id: request_id,
            content: GuestResponse::CommandOutput(response),
        };
        if tx.send(OrchestratorMessage::GuestResponse(response)).await.is_err() {
            break;
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
fn trim_terminator(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_terminator_keeps_trailing_whitespace() {
        assert_eq!(trim_terminator(b"a  \n"), b"a  ");
        assert_eq!(trim_terminator(b"a\t\r\n"), b"a\t");
        assert_eq!(trim_terminator(b"no terminator"), b"no terminator");
        assert_eq!(trim_terminator(b"\n"), b"");
        assert_eq!(trim_terminator(b""), b"");
    }
}
