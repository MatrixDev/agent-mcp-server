use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::SteamCommand;
use crate::tools::other::BPI_R4_DESTINATION;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BpiR4ScpTool {
    /// local path to copy from
    local_path: String,
    /// remote path to copy to
    remote_path: String,
}

impl BpiR4ScpTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let current_dir = context.resolve_path(".").await?;
        let target = format!("{BPI_R4_DESTINATION}:{}", self.remote_path);

        SteamCommand::new(request, "scp")
            .args(["-O", self.local_path.as_str(), target.as_str()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
