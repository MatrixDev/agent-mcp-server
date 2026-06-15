use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileMoveTool {
    /// path to file or directory being moved
    source: String,
    /// destination path, including file name
    target: String,
}

impl FileMoveTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let source = context.resolve_path(&self.source).await?;
        let target = context.resolve_path(&self.target).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &source).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &target).await?;

        if let Err(e) = tokio::fs::rename(&source, &target).await {
            let message = format!("failed to move {source:?} to {target:?}: {e}");
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }

        Ok(format!("moved {source:?} to {target:?}"))
    }
}
