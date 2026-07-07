use std::process::Stdio;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tokio::process::Command;
use tracing::error;

const SCRIPT: &str = "/home/matrixdev.guest/projects/ieee1905_bench.sh";

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct Ieee1905BenchTool {}

impl Ieee1905BenchTool {
    pub async fn handle(self) -> Result<String, ErrorData> {
        Command::new("cargo")
            .args(["build"])
            .args(["--package", "ieee1905"])
            .args(["--target", "aarch64-unknown-linux-musl"])
            .args(["--release"])
            .current_dir("/Users/matrixdev/Projects/gl/Comcast/ieee1905-rs/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .status()
            .await
            .map_err(|e| {
                let message = format!("cargo build failed: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let output = Command::new("limactl")
            .args(["shell", "default", "sudo", "timeout", "10s", SCRIPT])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| {
                let message = format!("failed to run ieee1905 benchmark: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(format!("STDOUT:\n{stdout}\n\nSTDERR:\n{stderr}"))
    }
}
