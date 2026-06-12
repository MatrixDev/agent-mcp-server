use std::num::NonZeroUsize;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileReadTool {
    /// path to file being read
    path: String,
    /// read starting from this line
    start_line: Option<usize>,
    /// read this amount of lines
    line_count: Option<NonZeroUsize>,
}

impl FileReadTool {
    pub async fn handle(self, context: &CargoRunner) -> Result<String, ErrorData> {
        let _ = context;

        let Ok(path) = tokio::fs::canonicalize(&self.path).await else {
            let message = format!("failed to canonicalize path: {}", self.path);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let result = contents
            .split_inclusive('\n')
            .skip(self.start_line.unwrap_or(0))
            .take(self.line_count.map_or(usize::MAX, NonZeroUsize::get))
            .collect::<String>();

        Ok(result)
    }
}
