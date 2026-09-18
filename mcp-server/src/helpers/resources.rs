use std::path::{Path, PathBuf};

use crate::helpers::errors::McpResult;
use crate::mcp_internal_error;

////////////////////////////////////////////////////////////////////////////////
// WARNING: resolved from the working directory, never from the executable. The server is
// launched from the same place whichever profile it was built with, so a debug run and a
// release run see the same mcp-resources.
pub async fn mcp_resource_path(relative: impl AsRef<Path>) -> McpResult<PathBuf> {
    let path = std::env::current_dir()
        .map_err(|e| mcp_internal_error!("no working directory: {e}"))?
        .join("mcp-resources")
        .join(relative);

    let path = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| mcp_internal_error!("failed to canonicalize resources folder: {e}"))?;

    match tokio::fs::try_exists(&path).await {
        Ok(true) => Ok(path),
        Ok(false) => Err(mcp_internal_error!("resource has a broken symlink {}", path.display())),
        Err(e) => Err(mcp_internal_error!("missing resource {}: {e}", path.display())),
    }
}
