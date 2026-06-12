use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileWriteTool {
    /// path to file being written
    path: String,
    /// contents written to the file, overwriting any existing file
    contents: String,
}

impl FileWriteTool {
    pub async fn handle(self, context: &CargoRunner) -> Result<String, ErrorData> {
        let _ = context;

        if tokio::fs::write(&self.path, &self.contents).await.is_err() {
            let message = format!("failed to write a file: {}", self.path);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("wrote {} bytes to {}", self.contents.len(), self.path))
    }
}
