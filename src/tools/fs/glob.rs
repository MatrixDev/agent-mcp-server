use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use glob::MatchOptions;
use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;

/// Cap on returned paths, so a broad pattern cannot flood the model context.
const MAX_RESULTS: usize = 1000;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GlobTool {
    /// glob pattern matched against paths relative to the search directory.
    /// Use `**` to cross directories, eg "**/*.rs", "src/**/*.ts" or "*.toml".
    #[serde(alias = "glob")]
    pattern: String,
    /// directory to search within, relative to the workspace; defaults to the whole workspace
    #[serde(default, alias = "directory")]
    path: Option<String>,
}

impl GlobTool {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let path = context.resolve_path(self.path.as_deref().unwrap_or(".")).await?;
        let workspace = context.resolve_path("../../..").await?;
        context.check_permissions(PermissionsGroup::FsRead, &path).await?;

        let mut results = Vec::new();
        for file in Self::collect(&path, Some(&self.pattern)).await? {
            let display = file.strip_prefix(&workspace).unwrap_or(&file);
            results.push(display.to_string_lossy().into_owned());
        }

        if results.is_empty() {
            return Ok(format!("no files match pattern {:?}", self.pattern));
        }

        let truncated = results.len() > MAX_RESULTS;
        results.truncate(MAX_RESULTS);

        let mut output = results.join("\n");
        if truncated {
            output.push_str(&format!("\n... results truncated at {MAX_RESULTS}"));
        }
        Ok(output)
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn collect(path: &Path, pattern: Option<&str>) -> Result<Vec<PathBuf>, ErrorData> {
        const MATCH_OPTIONS: MatchOptions = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };

        let pattern = match pattern {
            Some(e) => Some(glob::Pattern::new(e).map_err(|e| {
                let message = format!("invalid pattern: {e}");
                error!("{message}");
                ErrorData::invalid_request(message, None)
            })?),
            None => None,
        };

        let mut files = collect_files(path).await;
        if let Some(pattern) = pattern {
            files.retain(|file| {
                let file = file.strip_prefix(path).unwrap_or(&file);
                pattern.matches_path_with(file, MATCH_OPTIONS)
            });
        }
        Ok(files)
    }
}

///
/// Recursively collect readable, non-hidden files under `root`, sorted by path.
///
/// Dot-prefixed entries (`.git`, `.DS_Store`, ...) and anything the permission
/// deny list rejects are skipped silently, so a search never fails just because
/// it walked into something off-limits. Symlinks are not followed.
///
async fn collect_files(root: &Path) -> Vec<PathBuf> {
    if tokio::fs::metadata(&root).await.is_ok_and(|m| m.is_file()) {
        // a single file searches just that file, a directory walks recursively
        return vec![root.to_path_buf()];
    }

    let mut files = Vec::new();
    let mut dirs = VecDeque::from([root.to_path_buf()]);

    while let Some(dir) = dirs.pop_front() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            let path = entry.path();

            // file_type() does not follow symlinks, so symlinked dirs/files are ignored
            match entry.file_type().await {
                Ok(file_type) if file_type.is_dir() => dirs.push_back(path),
                Ok(file_type) if file_type.is_file() => files.push(path),
                _ => {}
            }
        }
    }
    files
}
