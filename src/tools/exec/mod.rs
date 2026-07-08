use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use rmcp::model::ProgressNotificationParam;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::error;

pub mod cargo;
pub mod gradle;

////////////////////////////////////////////////////////////////////////////////
#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

////////////////////////////////////////////////////////////////////////////////
async fn execute_command<I, S>(
    request: &RequestContext<RoleServer>,
    program: impl AsRef<Path>,
    arguments: I,
    current_dir: impl AsRef<Path>,
) -> Result<String, ErrorData>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new(program.as_ref())
        .args(arguments)
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
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

    let (tx, mut rx) = mpsc::channel::<(StreamKind, String)>(256);
    let stdout_reader = tokio::spawn(read_lines(stdout, StreamKind::Stdout, tx.clone()));
    let stderr_reader = tokio::spawn(read_lines(stderr, StreamKind::Stderr, tx));

    let progress_token = request.meta.get_progress_token();
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
            let _ = request.peer.notify_progress(param).await;
        }
    }

    let _ = stdout_reader.await;
    let _ = stderr_reader.await;
    let _ = child.wait().await;

    Ok(format!("STDOUT:\n{stdout_buf}\n\nSTDERR:\n{stderr_buf}"))
}

////////////////////////////////////////////////////////////////////////////////
async fn read_lines(
    reader: impl tokio::io::AsyncRead + Unpin,
    stream: StreamKind,
    tx: mpsc::Sender<(StreamKind, String)>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stream, line)).await.is_err() {
            break;
        }
    }
}
