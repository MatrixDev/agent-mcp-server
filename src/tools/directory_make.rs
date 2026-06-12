use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DirectoryMakeTool {
    /// path to directory being created, parents are created as needed
    path: String,
}

impl DirectoryMakeTool {
    pub async fn handle(self, context: &CargoRunner) -> Result<String, ErrorData> {
        let _ = context;

        if tokio::fs::create_dir_all(&self.path).await.is_err() {
            let message = format!("failed to create a directory: {}", self.path);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("created directory {}", self.path))
    }
}
