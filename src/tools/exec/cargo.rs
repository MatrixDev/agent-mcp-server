use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;

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

        StreamCommand::new(request, "cargo")
            .args(self.arguments)
            .current_dir(project_dir)
            .execute()
            .await
    }
}
