use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileEditTool {
    /// path to file being modified
    path: String,
    /// replace starting from this line
    start_line: usize,
    /// replace this amount of lines, zero inserts without removing any
    line_count: usize,
    /// text inserted in place of the removed lines
    new_text: String,
}

impl FileEditTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &path).await?;

        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let mut lines = contents.split_inclusive('\n');
        let mut buffer = String::new();

        buffer.extend(lines.by_ref().take(self.start_line));
        buffer.push_str(&self.new_text);
        if !self.new_text.is_empty() && !self.new_text.ends_with('\n') {
            buffer.push('\n');
        }
        buffer.extend(lines.skip(self.line_count));

        if let Err(e) = tokio::fs::write(&path, buffer).await {
            let message = format!("failed to write a file: {}\n{e}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("updated {}", path.display()))
    }
}
