mod task;

use std::collections::HashMap;
use std::ops::ControlFlow;

use mcp_qemu_agent_proto::{
    AgentPacket, CommandExec, CommandId, CommandKill, GenericResult, GuestResponse, HostRequest, Initialize,
    Initialized, PROTOCOL_VERSION, RequestId,
};
use tracing::{debug, info, instrument, warn};

use crate::CHANNEL_CAPACITY;
use crate::orchestrator::task::Task;

////////////////////////////////////////////////////////////////////////////////
enum OrchestratorMessage {
    GuestResponse(AgentPacket<GuestResponse>),
    TaskFinished(CommandId),
}

////////////////////////////////////////////////////////////////////////////////
pub struct Orchestrator {
    request_rx: tokio::sync::mpsc::Receiver<AgentPacket<HostRequest>>,
    response_tx: tokio::sync::mpsc::Sender<AgentPacket<GuestResponse>>,
    message_tx: tokio::sync::mpsc::Sender<OrchestratorMessage>,
    message_rx: tokio::sync::mpsc::Receiver<OrchestratorMessage>,
    session: Option<u64>,
    tasks: HashMap<CommandId, Task>,
}

impl Orchestrator {
    ////////////////////////////////////////////////////////////////////////////////
    #[instrument(skip_all, name = "orchestrator")]
    pub async fn run(
        request_rx: tokio::sync::mpsc::Receiver<AgentPacket<HostRequest>>,
        response_tx: tokio::sync::mpsc::Sender<AgentPacket<GuestResponse>>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
        let mut this = Self {
            request_rx,
            response_tx,
            message_tx: tx,
            message_rx: rx,
            session: None,
            tasks: Default::default(),
        };

        while this.run_loop_iteration().await.is_continue() {}
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn run_loop_iteration(&mut self) -> ControlFlow<()> {
        tokio::select! {
            request = self.request_rx.recv() => {
                return self.handle_host_request(request).await;
            },
            message = self.message_rx.recv() => {
                return self.handle_message(message).await;
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_host_request(&mut self, request: Option<AgentPacket<HostRequest>>) -> ControlFlow<()> {
        let Some(request) = request else {
            return ControlFlow::Break(debug!("reader channel closed"));
        };

        match &request.content {
            HostRequest::Initialize(e) => self.handle_initialize(request.id, e).await,
            HostRequest::Ping => self.handle_ping(request.id).await,
            HostRequest::CommandExec(e) => self.handle_command_exec(request.id, e).await,
            HostRequest::CommandKill(e) => self.handle_command_kill(request.id, e).await,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_initialize(&mut self, request_id: RequestId, request: &Initialize) -> ControlFlow<()> {
        let request_version = request.version;
        if request_version != PROTOCOL_VERSION {
            warn!("wrong protocol version {request_version}, supported {PROTOCOL_VERSION}");
        }

        if self.session != Some(request.session) {
            info!("session {:?} -> {}", self.session, request.session);

            let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
            self.session = Some(request.session);
            self.tasks.clear();
            self.message_tx = tx;
            self.message_rx = rx;
        }

        let response = AgentPacket {
            id: request_id,
            content: GuestResponse::Initialized(Initialized {
                version: PROTOCOL_VERSION,
                session: request.session,
            }),
        };
        self.send_response(response).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_ping(&mut self, request_id: RequestId) -> ControlFlow<()> {
        self.send_generic_response(request_id, true, "pong").await
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_command_exec(&mut self, request_id: RequestId, request: &CommandExec) -> ControlFlow<()> {
        if self.tasks.contains_key(&request.id) {
            let error = "a command with this id is already running";
            return self.send_generic_response(request_id, false, error).await;
        }

        match Task::spawn(request_id, request, self.message_tx.clone()) {
            Ok(task) => {
                self.tasks.insert(request.id, task);
                self.send_generic_response(request_id, true, "command started").await
            }
            Err(e) => self.send_generic_response(request_id, false, e.to_string()).await,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_command_kill(&mut self, request_id: RequestId, request: &CommandKill) -> ControlFlow<()> {
        match self.tasks.get(&request.id) {
            Some(task) => {
                task.kill();
                self.send_generic_response(request_id, true, "started").await
            }
            None => {
                let error = "no command with this id is running";
                self.send_generic_response(request_id, false, error).await
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_message(&mut self, message: Option<OrchestratorMessage>) -> ControlFlow<()> {
        let Some(message) = message else {
            return ControlFlow::Continue(());
        };

        match message {
            OrchestratorMessage::GuestResponse(response) => self.send_response(response).await,
            OrchestratorMessage::TaskFinished(id) => {
                self.tasks.remove(&id);
                ControlFlow::Continue(())
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn send_response(&mut self, response: AgentPacket<GuestResponse>) -> ControlFlow<()> {
        match self.response_tx.send(response).await {
            Ok(_) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(debug!("writer channel closed")),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn send_generic_response(
        &mut self,
        request_id: RequestId,
        success: bool,
        message: impl Into<String>,
    ) -> ControlFlow<()> {
        let response = AgentPacket {
            id: request_id,
            content: GuestResponse::GenericResult(GenericResult {
                success,
                message: message.into(),
            }),
        };
        self.send_response(response).await
    }
}
