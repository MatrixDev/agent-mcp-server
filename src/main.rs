mod context;
mod path_resolver;
mod permissions;
mod tools;

use std::sync::{Arc, OnceLock};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{InitializeRequestParams, InitializeResult};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler, ServiceExt};
use tokio::sync::Mutex;
use tools::exec::cargo_exec::CargoRunTool;
use tools::exec::gradle_exec::GradleRunTool;
use tools::fs::directory_list::DirectoryListTool;
use tools::fs::directory_make::DirectoryMakeTool;
use tools::fs::file_edit::FileEditTool;
use tools::fs::file_move::FileMoveTool;
use tools::fs::file_read::FileReadTool;
use tools::fs::file_write::FileWriteTool;
use tools::fs::glob::GlobTool;
use tools::fs::grep::GrepTool;
use tools::lights::lights_info::LightsInfoTool;
use tools::lights::lights_set_color::LightsSetColorTool;
use tracing::{error, info, instrument};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::context::McpAgentContext;
use crate::tools::ieee1905_bench::Ieee1905BenchTool;
use crate::tools::lights::controller::LightsController;

////////////////////////////////////////////////////////////////////////////////
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .init();

    if std::env::args().nth(1).as_deref() == Some("stdio") {
        serve_stdio().await?;
    } else {
        serve_http().await?;
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "serve_stdio")]
async fn serve_stdio() -> anyhow::Result<()> {
    let lights = LightsController::new()?;
    let service = McpAgentHandler::new(lights)?
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?;
    service.waiting().await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "serve_http")]
async fn serve_http() -> anyhow::Result<()> {
    let lights = LightsController::new()?;
    let service = StreamableHttpService::new(
        move || McpAgentHandler::new(lights.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;

    info!("streamable HTTP server listening");
    axum::serve(listener, router).await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
struct McpAgentHandler {
    lights: LightsController,
    context: Mutex<Option<Arc<McpAgentContext>>>,
    init_context: OnceLock<RequestContext<RoleServer>>,
}

impl McpAgentHandler {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new(lights: LightsController) -> std::io::Result<Self> {
        Ok(Self {
            lights,
            init_context: Default::default(),
            context: Default::default(),
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn do_initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        info!("client: {} v{}", request.client_info.name, request.client_info.version);
        self.init_context.get_or_init(|| context);
        Ok(self.get_info())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn try_get_context(&self) -> Result<Arc<McpAgentContext>, ErrorData> {
        let context_cell = &mut *self.context.lock().await;
        if let Some(context) = context_cell {
            return Ok(context.clone());
        }

        let init_context = self.init_context.get().ok_or_else(|| {
            let message = "init context is missing";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let context = McpAgentContext::new(init_context, self.lights.clone()).await?;
        Ok(context_cell.insert(Arc::new(context)).clone())
    }
}

#[tool_handler(router = Self::tool_router())]
impl ServerHandler for McpAgentHandler {
    #[instrument(skip_all, "initialize")]
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.do_initialize(request, context).await
    }
}

#[tool_router]
impl McpAgentHandler {
    ////////////////////////////////////////////////////////////////////////////////
    // File system
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Read a file, returns content range with numbered lines")]
    #[instrument(skip_all, "tool/read_file")]
    pub async fn read_file(&self, args: Parameters<FileReadTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Write a file, overwriting any existing contents")]
    #[instrument(skip_all, "tool/write_file")]
    pub async fn write_file(&self, args: Parameters<FileWriteTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Replace a range of lines in a file with new text")]
    #[instrument(skip_all, "tool/edit_file")]
    pub async fn edit_file(&self, args: Parameters<FileEditTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Move or rename a file or directory")]
    #[instrument(skip_all, "tool/move_file")]
    pub async fn move_file(&self, args: Parameters<FileMoveTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "List the entries of a directory")]
    #[instrument(skip_all, "tool/list_directory")]
    pub async fn list_directory(&self, args: Parameters<DirectoryListTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Create a directory, including parents")]
    #[instrument(skip_all, "tool/make_directory")]
    pub async fn make_directory(&self, args: Parameters<DirectoryMakeTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Find files by glob pattern, eg \"**/*.rs\" or \"src/*.toml\"")]
    #[instrument(skip_all, "tool/glob")]
    pub async fn glob(&self, args: Parameters<GlobTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Search file contents with a regular expression and return matching lines")]
    #[instrument(skip_all, "tool/grep")]
    pub async fn grep(&self, args: Parameters<GrepTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Benchmarks
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(
        description = "Run the ieee1905 release binary for 5s under `/usr/bin/time -v` and return its resource-usage report"
    )]
    #[instrument(skip_all, "tool/ieee1905_bench")]
    pub async fn ieee1905_bench(&self, args: Parameters<Ieee1905BenchTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        args.0.handle().await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Cargo
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `cargo fetch` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_fetch")]
    async fn cargo_fetch(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "fetch").await
    }

    #[tool(description = "Runs `cargo build` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_build")]
    async fn cargo_build(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "build").await
    }

    #[tool(description = "Runs `cargo test` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_test")]
    async fn cargo_test(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "test").await
    }

    #[tool(description = "Runs `cargo check` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_check")]
    async fn cargo_check(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "check").await
    }

    #[tool(description = "Runs `cargo clippy` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_clippy")]
    async fn cargo_clippy(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "clippy").await
    }

    #[tool(description = "Runs `cargo audit` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_audit")]
    async fn cargo_audit(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "audit").await
    }

    #[tool(description = "Runs `cargo deny` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo_deny")]
    async fn cargo_deny(&self, args: Parameters<CargoRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "deny").await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Gradle
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `gradle build` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle_build")]
    async fn gradle_build(&self, args: Parameters<GradleRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "build").await
    }

    #[tool(description = "Runs `gradle test` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle_test")]
    async fn gradle_test(&self, args: Parameters<GradleRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "test").await
    }

    #[tool(description = "Runs `gradle check` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle_check")]
    async fn gradle_check(&self, args: Parameters<GradleRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "check").await
    }

    #[tool(description = "Runs `gradle assemble` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle_assemble")]
    async fn gradle_assemble(&self, args: Parameters<GradleRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "assemble").await
    }

    #[tool(description = "Runs `gradle clean` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle_clean")]
    async fn gradle_clean(&self, args: Parameters<GradleRunTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, "clean").await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Playing
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Returns information about available smart lights")]
    #[instrument(skip_all, "tool/lights_info")]
    async fn lights_info(&self, args: Parameters<LightsInfoTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Sets smart light with provided id to requested color")]
    #[instrument(skip_all, "tool/lights_set_color")]
    async fn lights_set_color(&self, args: Parameters<LightsSetColorTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }
}
