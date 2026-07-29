use std::path::Path;

use base64::Engine;
use rmcp::ErrorData;
use rmcp::model::{AnnotateAble, Content, RawAudioContent, RawContent, ResourceContents};
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::helpers::mime::mime_type;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileReadRawTool {
    /// path to the binary file being read
    path: String,
}

impl FileReadRawTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<Content, ErrorData> {
        const MAX_RAW_BYTES: u64 = 10 * 1024 * 1024;

        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsRead, &path).await?;

        // Reject oversized files by their metadata before pulling any bytes into memory.
        if let Ok(metadata) = tokio::fs::metadata(&path).await
            && metadata.len() > MAX_RAW_BYTES
        {
            let message = format!("file is too large: {} bytes (max {MAX_RAW_BYTES})", metadata.len());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }

        let Ok(bytes) = tokio::fs::read(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let mime_type = mime_type(&path, &bytes);
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(into_content(&path, mime_type, data))
    }
}

////////////////////////////////////////////////////////////////////////////////
fn into_content(path: &Path, mime_type: String, data: String) -> Content {
    if mime_type.starts_with("image/") {
        Content::image(data, mime_type)
    } else if mime_type.starts_with("audio/") {
        RawContent::Audio(RawAudioContent { data, mime_type }).no_annotation()
    } else {
        let uri = format!("file://{}", path.display());
        Content::resource(ResourceContents::blob(data, uri).with_mime_type(mime_type))
    }
}
