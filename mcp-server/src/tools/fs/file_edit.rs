use std::num::NonZeroUsize;

use rmcp::ErrorData;
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileEditTool {
    /// path to file being modified
    path: String,
    /// 1-based line to start replacing at, inclusive
    start_line: NonZeroUsize,
    /// 1-based line to stop replacing at, exclusive.
    /// Defaults to `start_line`, which inserts without removing any line.
    end_line: Option<NonZeroUsize>,
    /// text inserted in place of the removed lines
    new_text: String,
}

impl FileEditTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsWrite, &path).await?;

        let start_line = self.start_line.get();
        let end_line = self.end_line.map_or(start_line, NonZeroUsize::get);

        let Some(lines_count) = end_line.checked_sub(start_line) else {
            let message = format!("end_line ({end_line}) must not be less than start_line ({start_line})",);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let mut lines = contents.split_inclusive('\n');
        let mut buffer = String::new();

        buffer.extend(lines.by_ref().take(self.start_line.get() - 1));
        buffer.push_str(&self.new_text);
        if !self.new_text.is_empty() && !self.new_text.ends_with('\n') {
            buffer.push('\n');
        }
        buffer.extend(lines.skip(lines_count));

        if let Err(e) = tokio::fs::write(&path, buffer).await {
            let message = format!("failed to write a file: {}\n{e}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("updated {}", path.display()))
    }
}
