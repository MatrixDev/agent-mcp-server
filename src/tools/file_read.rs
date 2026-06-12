use std::num::NonZeroUsize;

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
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
        let start_line = self.start_line.unwrap_or_default();
        let line_count = self.line_count.map_or(usize::MAX, |e| e.get());

        let Ok(path) = tokio::fs::canonicalize(&self.path).await else {
            let message = format!("failed to canonicalize path: {}", self.path);
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let Ok(file) = File::open(&path).await else {
            let message = format!("failed to open a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let mut reader = BufReader::new(file);
        let mut buffer = String::new();

        for _ in 0..start_line {
            if reader.read_line(&mut buffer).await.is_err() {
                let message = format!("start line {start_line} is out of file bounds: {}", path.display());
                error!("{message}");
                return Err(ErrorData::invalid_request(message, None));
            }
            buffer.clear();
        }

        for _ in 0..line_count {
            if reader.read_line(&mut buffer).await.is_err() {
                break;
            }
        }
        Ok(buffer)
    }
}
