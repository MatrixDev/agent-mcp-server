use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::SteamCommand;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GitDiffTool {
    /// directory inside the git repository, selects the repository when several are present
    folder: PathBuf,
    /// how many commits back the diff starts from, it is always taken against the current working tree.
    /// Defaults to 0, which shows only uncommitted changes to tracked files
    commit_depth: Option<u32>,
    /// optional git pathspecs limiting the diff, resolved relative to `folder`,
    /// eg ["src"], ["*.rs"] or [":(exclude)Cargo.lock"]. Defaults to the whole repository
    paths: Option<Vec<String>>,
}

impl GitDiffTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.folder).await?;
        let depth = format!("HEAD~{}", self.commit_depth.unwrap_or(0));

        let mut arguments = vec![String::from("diff"), depth];
        if let Some(paths) = self.paths {
            if !paths.is_empty() {
                arguments.push(String::from("--"));
                arguments.extend(paths);
            }
        }

        SteamCommand::new(request, "git")
            .args(arguments)
            .current_dir(project_dir)
            .execute()
            .await
    }
}
