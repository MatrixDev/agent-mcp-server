use std::ffi::OsStr;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;
use crate::permissions::PermissionsGroup;
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
        let source = context.resolve_path(self.local_path).await?;
        let target = format!("{BPI_R4_DESTINATION}:{}", self.remote_path);

        context.check_permissions(PermissionsGroup::FsRead, &source).await?;

        StreamCommand::new(request, "scp")
            .args([OsStr::new("-O"), source.as_ref(), target.as_ref()])
            .current_dir(current_dir)
            .execute()
            .await
    }
}
