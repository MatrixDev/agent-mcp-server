use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::StreamCommand;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GithubCliTool {
    /// directory inside the git repository, selects the repository when several are present
    folder: PathBuf,
    /// extra arguments added to gh subcommand
    arguments: Vec<String>,
}

impl GithubCliTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
        subcommand: &[&str],
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.folder).await?;

        StreamCommand::new(request, "gh")
            .args(subcommand)
            .args(self.arguments)
            .current_dir(project_dir)
            .execute()
            .await
    }
}
