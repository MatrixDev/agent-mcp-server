use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LimaBashTool {
    /// bash command to run inside the local lima instance
    command: String,
}

impl LimaBashTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let current_dir = context.resolve_path(".").await?;

        StreamCommand::new(request, "limactl")
            .args(["shell", "default"])
            .args(["bash", "-c", self.command.as_str()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
