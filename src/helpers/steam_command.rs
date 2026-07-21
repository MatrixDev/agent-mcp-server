use std::ffi::OsStr;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use rmcp::model::ProgressNotificationParam;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::error;

////////////////////////////////////////////////////////////////////////////////
pub struct SteamCommand<'a> {
    request: &'a RequestContext<RoleServer>,
    command: Command,
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

impl<'a> SteamCommand<'a> {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new(request: &'a RequestContext<RoleServer>, program: impl AsRef<OsStr>) -> Self {
        let mut command = Command::new(program);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        Self { request, command }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn args<Args>(mut self, arguments: Args) -> Self
    where
        Args: IntoIterator,
        Args::Item: AsRef<OsStr>,
    {
        self.command.args(arguments);
        self
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn current_dir(mut self, current_dir: impl AsRef<Path>) -> Self {
        self.command.current_dir(current_dir);
        self
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn execute(self) -> Result<String, ErrorData> {
        self.execute_for_result().await.map(|e| e.to_string())
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn execute_for_result(mut self) -> Result<SteamCommandResult, ErrorData> {
        let mut child = self.command.spawn().map_err(|e| {
            let message = format!("failed to execute command: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            let message = "failed to open stdout";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let stderr = child.stderr.take().ok_or_else(|| {
            let message = "failed to open stdin";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<(StreamKind, String)>(256);
        let stdout_reader = tokio::spawn(read_lines(stdout, StreamKind::Stdout, tx.clone()));
        let stderr_reader = tokio::spawn(read_lines(stderr, StreamKind::Stderr, tx));

        let progress_token = self.request.meta.get_progress_token();
        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut progress = 0.0;

        while let Some((stream, line)) = rx.recv().await {
            match stream {
                StreamKind::Stdout => {
                    stdout_buf.push_str(&line);
                    stdout_buf.push('\n');
                }
                StreamKind::Stderr => {
                    stderr_buf.push_str(&line);
                    stderr_buf.push('\n');
                }
            }

            if let Some(token) = progress_token.clone() {
                progress += 1.0;
                let param = ProgressNotificationParam::new(token, progress).with_message(line);
                let _ = self.request.peer.notify_progress(param).await;
            }
        }

        let _ = stdout_reader.await;
        let _ = stderr_reader.await;
        let _ = child.stdin.take();
        let _ = child.stdout.take();
        let _ = child.stderr.take();

        let status = child.wait().await.map_err(|e| {
            let message = format!("failed to wait for process to finish: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        Ok(SteamCommandResult {
            command: self.command,
            stdout: stdout_buf,
            stderr: stderr_buf,
            status,
        })
    }
}

////////////////////////////////////////////////////////////////////////////////
pub struct SteamCommandResult {
    command: Command,
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

impl SteamCommandResult {
    pub fn into_status_error(self) -> Result<Self, ErrorData> {
        if self.status.success() {
            return Ok(self);
        }

        let program = self.command.as_std().get_program();
        error!("{program:?} command failed: {}", self.status);
        Err(ErrorData::internal_error(self.to_string(), None))
    }
}

impl Display for SteamCommandResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RESULT: {}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
            self.status, self.stdout, self.stderr,
        )
    }
}

////////////////////////////////////////////////////////////////////////////////
async fn read_lines(
    reader: impl tokio::io::AsyncRead + Unpin,
    stream: StreamKind,
    tx: tokio::sync::mpsc::Sender<(StreamKind, String)>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stream, line)).await.is_err() {
            break;
        }
    }
}
