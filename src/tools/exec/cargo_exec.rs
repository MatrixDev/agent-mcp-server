use std::path::PathBuf;
use std::process::Stdio;

use rmcp::model::ProgressNotificationParam;
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::error;

use crate::context::McpAgentContext;

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CargoRunTool {
    /// cargo project root directory
    project_dir: PathBuf,
    /// extra arguments added to cargo subcommand
    arguments: Vec<String>,
}

impl CargoRunTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.project_dir).await?;

        let mut child = Command::new("cargo")
            .args(&self.arguments)
            .current_dir(project_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                let message = format!("failed to execute cargo: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        // Each reader forwards its lines over the channel as they arrive; the
        // channel closes once both readers have finished and dropped their sender.
        let (tx, mut rx) = mpsc::channel::<(Stream, String)>(256);
        let stdout_reader = tokio::spawn(read_lines(stdout, Stream::Stdout, tx.clone()));
        let stderr_reader = tokio::spawn(read_lines(stderr, Stream::Stderr, tx));

        let progress_token = request.meta.get_progress_token();
        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut progress = 0.0;

        while let Some((stream, line)) = rx.recv().await {
            match stream {
                Stream::Stdout => {
                    stdout_buf.push_str(&line);
                    stdout_buf.push('\n');
                }
                Stream::Stderr => {
                    stderr_buf.push_str(&line);
                    stderr_buf.push('\n');
                }
            }

            // Stream the line to the client as progress, if it asked for it.
            if let Some(token) = progress_token.clone() {
                progress += 1.0;
                let param = ProgressNotificationParam::new(token, progress).with_message(line);
                let _ = request.peer.notify_progress(param).await;
            }
        }

        let _ = stdout_reader.await;
        let _ = stderr_reader.await;
        let _ = child.wait().await;

        Ok(format!("STDOUT:\n{stdout_buf}\n\nSTDERR:\n{stderr_buf}"))
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn read_lines(reader: impl tokio::io::AsyncRead + Unpin, stream: Stream, tx: mpsc::Sender<(Stream, String)>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stream, line)).await.is_err() {
            break;
        }
    }
}
