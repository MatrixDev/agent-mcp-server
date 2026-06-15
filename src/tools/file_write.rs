use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileWriteTool {
    /// path to file being written
    path: String,
    /// contents written to the file, overwriting any existing file
    contents: String,
}

impl FileWriteTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &path).await?;

        if tokio::fs::write(&path, &self.contents).await.is_err() {
            let message = format!("failed to write a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("wrote {} bytes to {}", self.contents.len(), path.display()))
    }
}
