use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileMoveTool {
    /// path to file or directory being moved
    source: String,
    /// destination path
    destination: String,
}

impl FileMoveTool {
    pub async fn handle(self, context: &CargoRunner) -> Result<String, ErrorData> {
        let _ = context;

        let Ok(source) = tokio::fs::canonicalize(&self.source).await else {
            let message = format!("failed to canonicalize path: {}", self.source);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        if tokio::fs::rename(&source, &self.destination).await.is_err() {
            let message = format!("failed to move {} to {}", source.display(), self.destination);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("moved {} to {}", source.display(), self.destination))
    }
}
