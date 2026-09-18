use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;
use crate::tools::other::BPI_R4_DESTINATION;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BpiR4SshTool {
    /// bash command to run the device
    command: String,
}

impl BpiR4SshTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let current_dir = context.resolve_path(".").await?;

        StreamCommand::new(request, "ssh")
            .args([BPI_R4_DESTINATION, self.command.as_str()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
