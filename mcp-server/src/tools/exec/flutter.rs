use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FlutterRunTool {
    /// flutter project root directory
    project_dir: PathBuf,
    /// extra arguments added to flutter task
    arguments: Vec<String>,
}

impl FlutterRunTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
        command: &str,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.project_dir).await?;

        StreamCommand::new(request, "flutter")
            .args([command])
            .args(self.arguments)
            .current_dir(project_dir)
            .execute()
            .await
    }
}
