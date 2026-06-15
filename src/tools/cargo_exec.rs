use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tokio::process::Command;
use tracing::error;

use crate::context::McpAgentContext;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CargoRunTool {
    /// cargo project root directory
    project_dir: PathBuf,
    /// extra arguments added to cargo subcommand
    arguments: Vec<String>,
}

impl CargoRunTool {
    pub async fn handle(self, context: &McpAgentContext, subcommand: &str) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.project_dir).await?;
        let output = Command::new("cargo")
            .arg(subcommand)
            .args(&self.arguments)
            .current_dir(project_dir)
            .output()
            .await
            .map_err(|e| {
                let message = format!("failed to execute cargo: {e}");
                error!("{message}");
                ErrorData::internal_error(message, None)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(format!("STDOUT:\n{stdout}\n\nSTDERR:\n{stderr}"))
    }
}
