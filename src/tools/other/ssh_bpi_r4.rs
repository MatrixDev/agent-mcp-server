use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::SteamCommand;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SshBpiR4Tool {
    /// bash command to run the device
    command: String,
}

impl SshBpiR4Tool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let current_dir = context.resolve_path(".").await?;

        SteamCommand::new(request, "ssh")
            .args(["root@10.0.0.1", self.command.as_str()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
