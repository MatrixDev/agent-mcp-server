use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DirectoryListTool {
    /// path to directory being listed
    path: String,
}

impl DirectoryListTool {
    pub async fn handle(self, context: &CargoRunner) -> Result<String, ErrorData> {
        let _ = context;

        let Ok(path) = tokio::fs::canonicalize(&self.path).await else {
            let message = format!("failed to canonicalize path: {}", self.path);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(mut entries) = tokio::fs::read_dir(&path).await else {
            let message = format!("failed to read a directory: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let mut buffer = String::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            buffer.push_str(&entry.file_name().to_string_lossy());

            if entry.file_type().await.is_ok_and(|e| e.is_dir()) {
                buffer.push('/');
            }
            buffer.push('\n');
        }
        Ok(buffer)
    }
}
