mod context;
mod helpers;
mod path_resolver;
mod permissions;
mod tools;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};

use clap::{Parser, Subcommand};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Content, InitializeRequestParams, InitializeResult};
use rmcp::service::RequestContext;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use tokio::sync::Mutex;
use tracing::{error, info, instrument};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::context::McpAgentContext;
use crate::tools::exec::cargo::CargoRunTool;
use crate::tools::exec::git::GitRunTool;
use crate::tools::exec::gradle::GradleRunTool;
use crate::tools::fs::directory_list::DirectoryListTool;
use crate::tools::fs::directory_make::DirectoryMakeTool;
use crate::tools::fs::file_edit::FileEditTool;
use crate::tools::fs::file_move::FileMoveTool;
use crate::tools::fs::file_read::FileReadTool;
use crate::tools::fs::file_read_raw::FileReadRawTool;
use crate::tools::fs::file_write::FileWriteTool;
use crate::tools::fs::glob::GlobTool;
use crate::tools::fs::grep::GrepTool;
use crate::tools::lights::controller::LightsController;
use crate::tools::lights::lights_info::LightsInfoTool;
use crate::tools::lights::lights_set_color::LightsSetColorTool;
use crate::tools::other::bpi_r4_scp::BpiR4ScpTool;
use crate::tools::other::bpi_r4_ssh::BpiR4SshTool;
use crate::tools::other::ieee1905_bench::Ieee1905BenchTool;

////////////////////////////////////////////////////////////////////////////////
const DEFAULT_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEFAULT_PORT: u16 = 9999;

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Parser)]
#[command(version, about = "MDev MCP server")]
struct Arguments {
    #[command(subcommand)]
    transport: Option<Transport>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Debug, Subcommand)]
enum Transport {
    /// Serve over stdio, for clients that spawn the binary themselves
    Stdio,
    /// Serve streamable HTTP at /mcp, used when no subcommand is given
    Http {
        /// port to bind the listener to
        #[arg(short, long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
}

impl Default for Transport {
    fn default() -> Self {
        Self::Http { port: DEFAULT_PORT }
    }
}

////////////////////////////////////////////////////////////////////////////////
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE))
        .init();

    match arguments.transport.unwrap_or_default() {
        Transport::Stdio => serve_stdio().await?,
        Transport::Http { port } => serve_http(SocketAddr::new(DEFAULT_ADDRESS, port)).await?,
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
async fn serve_http(listen: SocketAddr) -> anyhow::Result<()> {
    let lights = LightsController::new()?;
    let service = StreamableHttpService::new(
        move || McpAgentHandler::new(lights.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(listen).await?;

    info!("streamable HTTP server listening on http://{listen}/mcp");
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

    #[tool(description = "Read any binary file (image, audio, blob) and return it base64-encoded with its mime type")]
    #[instrument(skip_all, "tool/read_file_raw")]
    pub async fn read_raw(&self, args: Parameters<FileReadRawTool>) -> Result<Content, ErrorData> {
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
    // Cargo
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `cargo` command directly without a terminal shell")]
    #[instrument(skip_all, "tool/cargo")]
    async fn cargo(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<CargoRunTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Gradle
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `gradle` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/gradle")]
    async fn gradle(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<GradleRunTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Git
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `git diff` directly without a terminal shell")]
    #[instrument(skip_all, "tool/git_diff")]
    async fn git_diff(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<GitRunTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request, "diff").await
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

    ////////////////////////////////////////////////////////////////////////////////
    // Other
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Run ieee1905 release binary for few seconds and return its resource-usage report")]
    #[instrument(skip_all, "tool/ieee1905_bench")]
    pub async fn ieee1905_bench(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<Ieee1905BenchTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    #[tool(description = "Run ssh command on the remote BPI-R4 device")]
    #[instrument(skip_all, "tool/bpi_r4_ssh")]
    pub async fn bpi_r4_ssh(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<BpiR4SshTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    #[tool(description = "Run scp command to copy local files to the remote BPI-R4 device")]
    #[instrument(skip_all, "tool/bpi_r4_scp")]
    pub async fn bpi_r4_scp(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<BpiR4ScpTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }
}
