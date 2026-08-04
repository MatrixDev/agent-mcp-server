use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::helpers::steam_command::SteamCommand;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GitRunTool {
    /// directory inside the git repository, selects the repository when several are present
    folder: PathBuf,
    /// extra arguments added to git subcommand
    arguments: Vec<String>,
}

impl GitRunTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
        subcommand: &str,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.folder).await?;

        SteamCommand::new(request, "git")
            .args([subcommand])
            .args(self.arguments)
            .current_dir(project_dir)
            .execute()
            .await
    }
}
