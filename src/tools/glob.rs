use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;

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
        let workspace = context.resolve_path(".").await?;
        context.check_permissions(PermissionsGroup::FsRead, &path).await?;

        let mut results = Vec::new();
        for file in Self::collect(&path, Some(&self.pattern)).await {
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

    pub async fn collect(path: &Path, pattern: Option<&str>) -> Vec<PathBuf> {
        let mut files = collect_files(path).await;
        if let Some(pattern) = pattern {
            files.retain(|file| {
                let file = file.strip_prefix(path).unwrap_or(&file);
                glob_match(pattern.as_bytes(), file.to_string_lossy().as_bytes())
            });
        }
        files
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

///
/// Match a `/`-separated path against a glob pattern.
///
/// - `?` matches a single character except `/`
/// - `*` matches any run of characters within one path segment (not `/`)
/// - `**` matches any run of characters across segments (including `/`); a
///   trailing slash (`**/`) also matches zero directories
///
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((&b'*', rest)) if rest.first() == Some(&b'*') => {
            let mut after = &rest[1..];
            if after.first() == Some(&b'/') {
                after = &after[1..];
            }
            (0..=text.len()).any(|i| glob_match(after, &text[i..]))
        }
        Some((&b'*', rest)) => {
            let segment_end = text.iter().position(|&c| c == b'/').unwrap_or(text.len());
            (0..=segment_end).any(|i| glob_match(rest, &text[i..]))
        }
        Some((&b'?', rest)) => matches!(text.first(), Some(&c) if c != b'/') && glob_match(rest, &text[1..]),
        Some((&c, rest)) => text.first() == Some(&c) && glob_match(rest, &text[1..]),
    }
}

////////////////////////////////////////////////////////////////////////////////
#[cfg(test)]
mod tests {
    use super::glob_match;

    fn matches(pattern: &str, path: &str) -> bool {
        glob_match(pattern.as_bytes(), path.as_bytes())
    }

    #[test]
    fn single_star_stays_within_a_segment() {
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "src/main.rs"));
        assert!(matches("src/*.toml", "src/x.toml"));
        assert!(!matches("src/*.toml", "src/a/x.toml"));
    }

    #[test]
    fn double_star_crosses_segments() {
        assert!(matches("**/*.rs", "main.rs"));
        assert!(matches("**/*.rs", "src/a/b.rs"));
        assert!(matches("src/**/*.rs", "src/b.rs"));
        assert!(matches("src/**/*.rs", "src/a/b.rs"));
        assert!(!matches("src/**/*.rs", "tests/a.rs"));
    }

    #[test]
    fn question_mark_and_literals() {
        assert!(matches("?.rs", "a.rs"));
        assert!(!matches("?.rs", "ab.rs"));
        assert!(matches("Cargo.toml", "Cargo.toml"));
        assert!(!matches("Cargo.toml", "Cargo.lock"));
    }
}
