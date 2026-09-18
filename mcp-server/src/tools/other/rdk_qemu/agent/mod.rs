mod command_exec;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mcp_qemu_agent_proto::{
    AgentPacket, CommandExec, CommandId, CommandKill, GuestResponse, HostRequest, Initialize, PROTOCOL_VERSION,
    RequestId,
};
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;
use crate::tools::other::rdk_qemu::agent::command_exec::CommandExecTask;

////////////////////////////////////////////////////////////////////////////////
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GUEST_SHELL: &str = "/bin/sh";

type Pending = Arc<parking_lot::Mutex<HashMap<RequestId, tokio::sync::mpsc::Sender<GuestResponse>>>>;

////////////////////////////////////////////////////////////////////////////////
pub struct RdkQemuAgent {
    writer: Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_request_id: AtomicU64,
    next_command_id: AtomicU64,
    _join_set: JoinSet<()>,
}

impl RdkQemuAgent {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn connect(socket: &Path) -> McpResult<Self> {
        let socket_name = socket.display();
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(socket)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(mcp_internal_error!("failed to connect to {socket_name}: {e}")),
            Err(_) => return Err(mcp_internal_error!("connecting to {socket_name} timed out")),
        };

        let (read_half, write_half) = stream.into_split();
        let pending = Pending::default();

        let mut join_set = JoinSet::new();
        join_set.spawn(pump_agent(read_half, pending.clone()));

        Ok(Self {
            writer: Mutex::new(write_half),
            pending,
            next_request_id: AtomicU64::new(0),
            next_command_id: AtomicU64::new(rand::random()),
            _join_set: join_set,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn initialize(&self, timeout: Duration) -> McpResult<()> {
        let session = rand::random();
        let request = HostRequest::Initialize(Initialize {
            version: PROTOCOL_VERSION,
            session,
        });

        let response = self
            .request(request, timeout)
            .await
            .map_err(|e| mcp_internal_error!("initialize failed: {e}"))?;

        let GuestResponse::Initialized(response) = response else {
            return Err(mcp_internal_error!("invalid initialize response"));
        };

        let guest_version = response.version;
        if guest_version != PROTOCOL_VERSION {
            warn!("guest agent speaks protocol {guest_version}, current {PROTOCOL_VERSION}");
        }

        if response.session != session {
            return Err(mcp_internal_error!("invalid initialize response session"));
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn ping(&self, timeout: Duration) -> bool {
        self.request(HostRequest::Ping, timeout).await.is_ok()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn execute(
        &self,
        request: &RequestContext<RoleServer>,
        command: String,
        timeout: Duration,
    ) -> McpResult<String> {
        CommandExecTask::run(request, self, command, timeout).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn cancel(&self, command_id: CommandId) {
        let content = HostRequest::CommandKill(CommandKill { id: command_id });

        if let Err(e) = self.send(self.next_request_id(), content).await {
            warn!("failed to cancel {command_id:?}: {e:?}");
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn power_off(&self) -> McpResult<()> {
        let content = HostRequest::CommandExec(CommandExec {
            id: self.next_command_id(),
            command: GUEST_SHELL.to_string(),
            arguments: vec!["-c".to_string(), "poweroff".to_string()],
        });

        self.send(self.next_request_id(), content).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn register(&self, capacity: usize) -> (RequestId, tokio::sync::mpsc::Receiver<GuestResponse>) {
        let id = self.next_request_id();
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);

        self.pending.lock().insert(id, tx);
        (id, rx)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn unregister(&self, id: RequestId) {
        self.pending.lock().remove(&id);
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn next_command_id(&self) -> CommandId {
        CommandId(self.next_command_id.fetch_add(1, Ordering::Relaxed))
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn request(&self, content: HostRequest, timeout: Duration) -> McpResult<GuestResponse> {
        let (request_id, mut response_rx) = self.register(1);
        let _guard = scopeguard::guard(request_id, |e| self.unregister(e));

        self.send(request_id, content)
            .await
            .map_err(|e| mcp_internal_error!("failed to send an agent request: {e:?}"))?;

        tokio::time::timeout(timeout, response_rx.recv())
            .await
            .map_err(|e| mcp_internal_error!("agent request has timed out: {e:?}"))?
            .ok_or_else(|| mcp_internal_error!("agent has died"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn send(&self, id: RequestId, content: HostRequest) -> McpResult<()> {
        let packet = mcp_qemu_agent_proto::serialize_cobs(&AgentPacket { id, content })
            .map_err(|e| mcp_internal_error!("failed to serialize an agent request: {e}"))?;

        let mut writer = self.writer.lock().await;

        writer
            .write_all(&packet)
            .await
            .map_err(|e| mcp_internal_error!("agent write failed: {e}"))?;

        writer
            .flush()
            .await
            .map_err(|e| mcp_internal_error!("agent flush failed: {e}"))?;

        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn pump_agent(reader: OwnedReadHalf, pending: Pending) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::with_capacity(4096);

    loop {
        buffer.clear();

        match reader.read_until(0, &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => break warn!("RDK qemu agent read failed: {e}"),
        }

        match mcp_qemu_agent_proto::deserialize_cobs::<AgentPacket<GuestResponse>>(&mut buffer) {
            Ok(packet) => {
                // WARNING: cloned out from under the lock rather than sent to while holding it.
                // This send blocks once the waiter's channel is full, and the waiter takes the
                // same lock to unregister itself, so holding it across send is a deadlock.
                let responses = pending.lock().get(&packet.id).cloned();

                // WARNING: the waiter is gone whenever its tool call returned early, which is the normal
                // outcome of a cancellation, so output arriving late is dropped rather than reported
                if let Some(responses) = responses {
                    let _ = responses.send(packet.content).await;
                }
            }
            Err(e) => warn!("failed to decode a {} byte agent frame: {e:?}", buffer.len()),
        }
    }

    info!("RDK qemu agent channel closed");
    pending.lock().clear();
}
