use std::num::NonZeroUsize;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileReadTool {
    /// path to file being read
    path: String,
    /// 1-based line number to start at (lines, not bytes). Defaults to the start of the file.
    offset: Option<NonZeroUsize>,
    /// Maximum number of lines to return (lines, not bytes). Defaults to the whole file.
    limit: Option<NonZeroUsize>,
}

impl FileReadTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsRead, &path).await?;

        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let result = contents
            .split_inclusive('\n')
            .enumerate()
            .skip(self.offset.map_or(0, |e| e.get() - 1))
            .take(self.limit.map_or(usize::MAX, NonZeroUsize::get))
            .map(|(index, line)| format!("{:>6} {line}", index + 1))
            .collect::<String>();

        Ok(result)
    }
}
