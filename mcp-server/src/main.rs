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
use crate::tools::exec::flutter::FlutterRunTool;
use crate::tools::exec::git::GitRunTool;
use crate::tools::exec::github::GithubCliTool;
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
use crate::tools::keynote::keynote_add_code::KeynoteAddCodeTool;
use crate::tools::keynote::keynote_add_image::KeynoteAddImageTool;
use crate::tools::keynote::keynote_add_line::KeynoteAddLineTool;
use crate::tools::keynote::keynote_add_shape::KeynoteAddShapeTool;
use crate::tools::keynote::keynote_add_slides::KeynoteAddSlidesTool;
use crate::tools::keynote::keynote_add_text_box::KeynoteAddTextBoxTool;
use crate::tools::keynote::keynote_batch::KeynoteBatchTool;
use crate::tools::keynote::keynote_create::KeynoteCreateTool;
use crate::tools::keynote::keynote_delete_objects::KeynoteDeleteObjectsTool;
use crate::tools::keynote::keynote_delete_slides::KeynoteDeleteSlidesTool;
use crate::tools::keynote::keynote_edit_object::KeynoteEditObjectTool;
use crate::tools::keynote::keynote_edit_slide::KeynoteEditSlideTool;
use crate::tools::keynote::keynote_export::KeynoteExportTool;
use crate::tools::keynote::keynote_info::KeynoteInfoTool;
use crate::tools::keynote::keynote_move_slide::KeynoteMoveSlideTool;
use crate::tools::lights::controller::LightsController;
use crate::tools::lights::lights_info::LightsInfoTool;
use crate::tools::lights::lights_set_color::LightsSetColorTool;
use crate::tools::other::bpi_r4_scp::BpiR4ScpTool;
use crate::tools::other::bpi_r4_ssh::BpiR4SshTool;
use crate::tools::other::lima_bash::LimaBashTool;
use crate::tools::other::rdk_docker_bash::RdkDockerBashTool;
use crate::tools::other::rdk_qemu::{RdkQemuExecTool, RdkQemuManager, RdkQemuStartTool, RdkQemuStopTool};

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
    let service = McpAgentHandler::new(lights, RdkQemuManager::default())?
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?;
    service.waiting().await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
#[instrument(skip_all, "serve_http")]
async fn serve_http(listen: SocketAddr) -> anyhow::Result<()> {
    let lights = LightsController::new()?;
    // shared across sessions, the guest ssh port allows a single qemu instance at a time
    let qemu = RdkQemuManager::default();
    let service = StreamableHttpService::new(
        move || McpAgentHandler::new(lights.clone(), qemu.clone()),
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
    qemu: RdkQemuManager,
    context: Mutex<Option<Arc<McpAgentContext>>>,
    init_context: OnceLock<RequestContext<RoleServer>>,
}

impl McpAgentHandler {
    ////////////////////////////////////////////////////////////////////////////////
    pub fn new(lights: LightsController, qemu: RdkQemuManager) -> std::io::Result<Self> {
        Ok(Self {
            lights,
            qemu,
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

    #[tool(description = "Replace the line range [start_line, end_line) with new text, \
                       both 1-based, end_line exclusive and optional (omit it to insert)")]
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
    // Flutter
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `flutter build` task directly without a terminal shell")]
    #[instrument(skip_all, "tool/flutter_build")]
    async fn flutter_build(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<FlutterRunTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request, "build").await
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
    // GitHub CLI
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(description = "Runs `gh issue view` directly without a terminal shell")]
    #[instrument(skip_all, "tool/gh_issue_view")]
    async fn gh_issue_view(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<GithubCliTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request, &["issue", "view"]).await
    }

    #[tool(description = "Runs `gh pr view` directly without a terminal shell")]
    #[instrument(skip_all, "tool/gh_pr_view")]
    async fn gh_pr_view(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<GithubCliTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request, &["pr", "view"]).await
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
    // Keynote
    ////////////////////////////////////////////////////////////////////////////////

    #[tool(
        description = "Create a new Keynote presentation (*.key) using a template's theme, with its slides \
                       made from the theme layouts, including transitions and builds (animations)"
    )]
    #[instrument(skip_all, "tool/keynote_create")]
    async fn keynote_create(&self, args: Parameters<KeynoteCreateTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Describe a Keynote presentation: slide size, theme layouts with their placeholders, and \
                       every slide with its layout, background, title, body, notes, skipped flag, transition, \
                       builds (animations) and objects (id, kind, frame, text, fill, stroke, image). `slides` \
                       limits it to a range, `summary` gives one line per slide"
    )]
    #[instrument(skip_all, "tool/keynote_info")]
    async fn keynote_info(&self, args: Parameters<KeynoteInfoTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Add slides to a Keynote presentation, each made from a theme layout or duplicated \
                       from a given slide, with styled title and body, background, placeholder visibility, \
                       transitions and builds (animations). Every slide is checked before anything changes"
    )]
    #[instrument(skip_all, "tool/keynote_add_slides")]
    async fn keynote_add_slides(&self, args: Parameters<KeynoteAddSlidesTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Edit a slide of a Keynote presentation: title and body (plain or styled), presenter notes, \
                       skipped flag, background (colour, gradient, image or the layout's), shown placeholders, \
                       transition to the next slide and builds (animations) of its objects"
    )]
    #[instrument(skip_all, "tool/keynote_edit_slide")]
    async fn keynote_edit_slide(&self, args: Parameters<KeynoteEditSlideTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Move a slide of a Keynote presentation to another position")]
    #[instrument(skip_all, "tool/keynote_move_slide")]
    async fn keynote_move_slide(&self, args: Parameters<KeynoteMoveSlideTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Delete slides from a Keynote presentation")]
    #[instrument(skip_all, "tool/keynote_delete_slides")]
    async fn keynote_delete_slides(&self, args: Parameters<KeynoteDeleteSlidesTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Place an image (png, jpg, gif, heic, tiff, pdf, svg) on a Keynote slide: frame, fit \
                       (contain, cover, stretch, original), opacity, rounded corners, rotation, border, shadow, \
                       alt text, or into the layout's media placeholder. Returns its object id"
    )]
    #[instrument(skip_all, "tool/keynote_add_image")]
    async fn keynote_add_image(&self, args: Parameters<KeynoteAddImageTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Add a shape to a Keynote slide: rect, rounded_rect, ellipse, triangle, diamond, polygon, \
                       star, arrow, chevron, callout or an SVG path, with fill (colour or gradient), line, \
                       shadow, opacity and optional styled text inside. Returns its object id"
    )]
    #[instrument(skip_all, "tool/keynote_add_shape")]
    async fn keynote_add_shape(&self, args: Parameters<KeynoteAddShapeTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Add a free text box to a Keynote slide with styled text (runs with font, size, colour, bold, \
                       links…, or Markdown-lite), paragraph alignment, bullets and spacing, padding, vertical \
                       alignment, columns, shrink-to-fit and an optional fill or line. Returns its object id"
    )]
    #[instrument(skip_all, "tool/keynote_add_text_box")]
    async fn keynote_add_text_box(&self, args: Parameters<KeynoteAddTextBoxTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Draw a straight line on a Keynote slide from one point to another, with colour, width, dash \
                       and arrow heads. Returns its object id"
    )]
    #[instrument(skip_all, "tool/keynote_add_line")]
    async fn keynote_add_line(&self, args: Parameters<KeynoteAddLineTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Add a syntax-highlighted code block to a Keynote slide, from `code` or a source `file`: \
                       language, colour theme, font, size, line height, highlighted (others dimmed) lines and line \
                       numbers. A \"By Bullet\" build on it reveals the code line by line. Returns its object id"
    )]
    #[instrument(skip_all, "tool/keynote_add_code")]
    async fn keynote_add_code(&self, args: Parameters<KeynoteAddCodeTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Change any object of a Keynote presentation by its id: frame, rotation, z order, fill, line, \
                       shadow, opacity, text, text layout, lock, alt text or link"
    )]
    #[instrument(skip_all, "tool/keynote_edit_object")]
    async fn keynote_edit_object(&self, args: Parameters<KeynoteEditObjectTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(description = "Delete objects of a Keynote presentation by id, with their builds")]
    #[instrument(skip_all, "tool/keynote_delete_objects")]
    async fn keynote_delete_objects(&self, args: Parameters<KeynoteDeleteObjectsTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Render Keynote slides to PNG or SVG previews (drawn without Keynote, close but not \
                       identical) to check layout, overflowing text and colours; also reports text that doesn't \
                       fit its frame"
    )]
    #[instrument(skip_all, "tool/keynote_export")]
    async fn keynote_export(&self, args: Parameters<KeynoteExportTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    #[tool(
        description = "Apply many Keynote operations (add_slides, edit_slide, move_slide, delete_slides, add_image, \
                       add_shape, add_text_box, add_line, add_code, edit_object, delete_objects) in one call and \
                       one save; nothing is saved when one fails. Add operations take a `ref` name later \
                       operations use as \"$name\" for ids, build targets and z orders. `dry_run` checks without \
                       saving"
    )]
    #[instrument(skip_all, "tool/keynote_batch")]
    async fn keynote_batch(&self, args: Parameters<KeynoteBatchTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context).await
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Other
    ////////////////////////////////////////////////////////////////////////////////

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

    #[tool(description = "Run bash command on the locally running RDK docker instance")]
    #[instrument(skip_all, "tool/rdk_docker_bash")]
    pub async fn rdk_docker_bash(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<RdkDockerBashTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    #[tool(description = "Run bash command on the locally running lima instance")]
    #[instrument(skip_all, "tool/lima_bash")]
    pub async fn lima_bash(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<LimaBashTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request).await
    }

    #[tool(description = "Start the RDK qemu instance and wait until its guest command agent answers")]
    #[instrument(skip_all, "tool/rdk_qemu_start")]
    pub async fn rdk_qemu_start(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<RdkQemuStartTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        let context = self.try_get_context().await?;
        args.0.handle(&context, &request, &self.qemu).await
    }

    #[tool(description = "Run shell command on the running RDK qemu instance over its virtio-serial channel")]
    #[instrument(skip_all, "tool/rdk_qemu_exec")]
    pub async fn rdk_qemu_exec(
        &self,
        request: RequestContext<RoleServer>,
        args: Parameters<RdkQemuExecTool>,
    ) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        args.0.handle(&request, &self.qemu).await
    }

    #[tool(description = "Stop the running RDK qemu instance")]
    #[instrument(skip_all, "tool/rdk_qemu_stop")]
    pub async fn rdk_qemu_stop(&self, args: Parameters<RdkQemuStopTool>) -> Result<String, ErrorData> {
        info!("started: {args:#?}");
        args.0.handle(&self.qemu).await
    }
}
