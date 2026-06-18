use std::path::Path;

use regex::{Regex, RegexBuilder};
use rmcp::schemars::{self, JsonSchema};
use rmcp::ErrorData;
use serde::Deserialize;
use tracing::error;

use crate::context::McpAgentContext;
use crate::permissions::PermissionsGroup;
use crate::tools::fs::glob::GlobTool;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepTool {
    /// regular expression to search for, eg "fn\\s+\\w+"
    #[serde(alias = "pattern", alias = "regex", alias = "search")]
    query: String,
    /// file or directory to search within, relative to the workspace; defaults to the whole workspace
    #[serde(default, alias = "directory")]
    path: Option<String>,
    /// optional glob limiting which files are searched, eg "**/*.rs"
    #[serde(default, alias = "glob")]
    include: Option<String>,
    /// match case-insensitively, disabled by default
    #[serde(default, alias = "case_insensitive")]
    ignore_case: Option<bool>,
}

impl GrepTool {
    const MAX_RESULTS: usize = 1000;

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        if self.query.is_empty() {
            let message = "query must not be empty";
            error!("{message}");
            return Err(ErrorData::invalid_request(message, None));
        }
        self.handle_internal(context).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn handle_internal(self, context: &McpAgentContext) -> Result<String, ErrorData> {
        let root = context.resolve_path(self.path.as_deref().unwrap_or(".")).await?;
        let workspace = context.resolve_path("../../..").await?;
        context.check_permissions(PermissionsGroup::FsRead, &root).await?;

        let query = Self::build_query_regex(&self.query, self.ignore_case.unwrap_or(false))?;
        let files = GlobTool::collect(&root, self.include.as_deref()).await;

        let mut results = Vec::new();
        for file in files {
            if results.len() >= Self::MAX_RESULTS {
                break;
            }

            let display = file.strip_prefix(&workspace).unwrap_or(&file);
            let display = display.to_string_lossy().into_owned();

            if let Some(query) = query.as_ref() {
                if context.check_permissions(PermissionsGroup::FsRead, &file).await.is_ok() {
                    Self::collect_file_lines(&file, &display, query, &mut results).await;
                }
            } else {
                results.push(display);
            }
        }

        if results.is_empty() {
            return if self.query.is_empty() {
                Ok("no matches".to_string())
            } else {
                Ok(format!("no matches for {:?}", self.query))
            };
        }

        let mut output = results.join("\n");
        if results.len() >= Self::MAX_RESULTS {
            output.push_str(&format!("\n... results truncated at {}", Self::MAX_RESULTS));
        }
        Ok(output)
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn build_query_regex(query: &str, ignore_case: bool) -> Result<Option<Regex>, ErrorData> {
        if query.is_empty() {
            return Ok(None);
        }

        let query = RegexBuilder::new(query)
            .case_insensitive(ignore_case)
            .build()
            .map_err(|e| {
                let message = format!("invalid regular expression {query:?}: {e}");
                error!("{message}");
                ErrorData::invalid_request(message, None)
            })?;

        Ok(Some(query))
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn collect_file_lines(file: &Path, display: &str, query: &Regex, target: &mut Vec<String>) {
        let Ok(contents) = tokio::fs::read_to_string(&file).await else {
            return; // skip binary / non-utf8 / unreadable files
        };

        for (index, line) in contents.lines().enumerate() {
            if !query.is_match(line) {
                continue;
            }

            target.push(format!("{display}:{}:{}", index + 1, line.trim_end()));

            if target.len() >= Self::MAX_RESULTS {
                break;
            }
        }
    }
}
