use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DockerBashTool {
    /// bash command to run inside the RDK docker container
    command: String,
}

impl DockerBashTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let current_dir = context.resolve_path(".").await?;

        StreamCommand::new(request, "docker")
            .args(["exec", "rdkb-build-container"])
            .args(["bash", "-c", self.command.as_str()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
