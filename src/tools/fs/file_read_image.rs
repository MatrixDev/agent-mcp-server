use std::path::Path;

use base64::Engine;
use rmcp::model::Content;
use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileReadImageTool {
    /// path to the image file being read
    path: String,
}

impl FileReadImageTool {
    pub async fn handle(self, context: &McpAgentContext) -> Result<Content, ErrorData> {
        const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

        let path = context.resolve_path(&self.path).await?;
        context.check_permissions(PermissionsGroup::FsRead, &path).await?;

        let mime_type = mime_type_for(&path).ok_or_else(|| {
            let message = format!("unsupported image type: {}", path.display());
            error!("{message}");
            ErrorData::invalid_request(message, None)
        })?;

        // Reject oversized images by their metadata before pulling any bytes into memory.
        if let Ok(metadata) = tokio::fs::metadata(&path).await {
            if metadata.len() > MAX_IMAGE_BYTES {
                let message = format!("image is too large: {} bytes (max {MAX_IMAGE_BYTES})", metadata.len());
                error!("{message}");
                return Err(ErrorData::invalid_request(message, None));
            }
        }

        let Ok(bytes) = tokio::fs::read(&path).await else {
            let message = format!("failed to read a file: {}", path.display());
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        };

        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Content::image(data, mime_type))
    }
}

////////////////////////////////////////////////////////////////////////////////
fn mime_type_for(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        _ => return None,
    };
    Some(mime)
}
