use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::CargoRunner;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileEditTool {
    /// path to file being edited
    path: String,
    /// text to be replaced, must occur exactly once in the file
    old_string: String,
    /// text that replaces old_string
    new_string: String,
}

impl FileEditTool {
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

        let matches = contents.matches(&self.old_string).take(2).count();
        if matches != 1 {
            let message = format!("old_string to occur non-1 amount of times: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }

        let contents = contents.replace(&self.old_string, &self.new_string);
        if tokio::fs::write(&path, contents).await.is_err() {
            let message = format!("failed to write a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        Ok(format!("edited {}", path.display()))
    }
}
