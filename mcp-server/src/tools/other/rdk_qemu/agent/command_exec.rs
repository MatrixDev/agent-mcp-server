use std::pin::pin;
use std::time::Duration;

use mcp_qemu_agent_proto::{
    CommandFinished, CommandId, CommandOutput, CommandStream, GenericResult, GuestResponse, HostRequest, RequestId,
};
use rmcp::RoleServer;
use rmcp::model::ProgressNotificationParam;
use rmcp::service::RequestContext;
use tracing::warn;

use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;
use crate::tools::other::rdk_qemu::agent::{GUEST_SHELL, RdkQemuAgent};

////////////////////////////////////////////////////////////////////////////////
pub struct CommandExecTask<'a> {
    request: &'a RequestContext<RoleServer>,
    agent: &'a RdkQemuAgent,
    request_id: RequestId,
    command_id: CommandId,
    events: tokio::sync::mpsc::Receiver<GuestResponse>,
    command: String,
    timeout: Duration,
    command_stdout: String,
    command_stderr: String,
    progress_line: String,
}

impl<'a> CommandExecTask<'a> {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn run(
        request: &'a RequestContext<RoleServer>,
        agent: &'a RdkQemuAgent,
        command: String,
        timeout: Duration,
    ) -> McpResult<String> {
        let command_id = agent.next_command_id();
        let (request_id, events) = agent.register(256);
        let _guard = scopeguard::guard(request_id, |e| agent.unregister(e));

        let this = Self {
            agent,
            request_id,
            command_id,
            request,
            events,
            command,
            timeout,
            command_stdout: String::new(),
            command_stderr: String::new(),
            progress_line: String::new(),
        };

        this.run_inner().await
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn run_inner(mut self) -> McpResult<String> {
        let content = HostRequest::CommandExec(mcp_qemu_agent_proto::CommandExec {
            id: self.command_id,
            command: GUEST_SHELL.to_string(),
            arguments: vec!["-c".to_string(), self.command.to_string()],
        });

        self.agent.send(self.request_id, content).await?;

        let progress_token = self.request.meta.get_progress_token();
        let mut progress = 0.0;
        let mut progress_interval = tokio::time::interval(Duration::from_secs(1));
        let mut deadline = pin!(tokio::time::sleep(self.timeout));

        loop {
            tokio::select! {
                response = self.events.recv() =>{
                    if let Some(result) = self.handle_response(response).await? {
                        return Ok(result);
                    }
                },

                _ = progress_interval.tick() => {
                    if let Some(token) = progress_token.as_ref() && !self.progress_line.is_empty() {
                        progress += 0.1;
                        let param = ProgressNotificationParam::new(token.clone(), progress);
                        let param = param.with_message(self.progress_line.clone());
                        let _ = self.request.peer.notify_progress(param).await;
                    }
                }

                _ = &mut deadline => {
                    self.agent.cancel(self.command_id).await;
                    let timeout = self.timeout.as_secs();
                    return Ok(self.format_result(format!("timed out after {timeout}s")));
                }

                _ = self.request.ct.cancelled() => {
                    self.agent.cancel(self.command_id).await;
                    return Ok(self.format_result("cancelled by client"));
                }
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_response(&mut self, response: Option<GuestResponse>) -> McpResult<Option<String>> {
        let Some(response) = response else {
            return Err(mcp_internal_error!("QEMU agent channel closed"));
        };

        match response {
            GuestResponse::CommandOutput(e) => {
                self.handle_command_output(e).await;
                Ok(None)
            }
            GuestResponse::CommandFinished(e) => {
                let result = self.handle_command_finished(e).await;
                Ok(Some(result))
            }
            GuestResponse::GenericResult(e) => {
                self.handle_generic_result(e).await?;
                Ok(None)
            }
            _ => {
                warn!("ignoring an unexpected agent response : {response:?}");
                Ok(None)
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_command_output(&mut self, model: CommandOutput) {
        self.progress_line = String::from_utf8_lossy(&model.line).into_owned();

        let target = match model.stream {
            CommandStream::Stdout => &mut self.command_stdout,
            CommandStream::Stderr => &mut self.command_stderr,
        };

        target.push_str(&self.progress_line);
        target.push('\n');
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_command_finished(&mut self, model: CommandFinished) -> String {
        let result = match (model.code, model.signal) {
            (Some(code), _) => format!("exit status: {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => "unknown exit status".to_string(),
        };
        self.format_result(result)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn handle_generic_result(&mut self, model: GenericResult) -> McpResult<()> {
        if model.success {
            return Ok(());
        }
        let message = model.message;
        Err(mcp_internal_error!("failed to start the command: {message}"))
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn format_result(&self, message: impl Into<String>) -> String {
        let stdout = &self.command_stdout;
        let stderr = &self.command_stderr;
        format!("RESULT: {}\n\nSTDOUT:\n{stdout}\n\nSTDERR:\n{stderr}", message.into())
    }
}
