use std::path::PathBuf;

use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde::Deserialize;

use crate::context::McpAgentContext;
use crate::tools::exec::execute_command;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GradleRunTool {
    /// gradle project root directory
    project_dir: PathBuf,
    /// extra arguments added to gradle task
    arguments: Vec<String>,
}

impl GradleRunTool {
    pub async fn handle(
        self,
        context: &McpAgentContext,
        request: &RequestContext<RoleServer>,
    ) -> Result<String, ErrorData> {
        let project_dir = context.resolve_path(&self.project_dir).await?;
        let program = project_dir.join("gradlew");

        execute_command(request, program, self.arguments, project_dir).await
    }
}
