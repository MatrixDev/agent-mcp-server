use std::pin::pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail};
use rmcp::model::ProgressNotificationParam;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::tools::other::rdk_qemu::agent::RdkQemuAgent;
use crate::tools::other::rdk_qemu::console_buffer::RdkQemuConsoleBuffer;
use crate::tools::other::rdk_qemu::files::RdkQemuFiles;
use crate::tools::other::rdk_qemu::{AGENT_BINARY, AGENT_LIVENESS_TIMEOUT, CONSOLE_PROMPT_READY};

////////////////////////////////////////////////////////////////////////////////
const AGENT_PORT: &str = "rdk.agent0";
const AGENT_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const AGENT_MOUNT_TAG: &str = "rdkagent";

// a guest power off completes in ~16s, the rest is headroom for one that is still booting
const GUEST_POWER_OFF_TIMEOUT: Duration = Duration::from_secs(60);
const SIGTERM_TIMEOUT: Duration = Duration::from_secs(10);

////////////////////////////////////////////////////////////////////////////////
pub struct RdkQemuInstance {
    console: RdkQemuConsoleBuffer,
    child: Child,
    agent: Arc<RdkQemuAgent>,
    pumps: JoinSet<()>,
    _stdin: ChildStdin,
    _files: RdkQemuFiles,
}

impl RdkQemuInstance {
    ////////////////////////////////////////////////////////////////////////////////
    pub async fn spawn(
        request: &RequestContext<RoleServer>,
        timeout: Duration,
        files: RdkQemuFiles,
    ) -> Result<Self, ErrorData> {
        let wic_qcow2 = files.wic_qcow2().to_string_lossy();
        let u_boot_bin = files.u_boot_bin().to_string_lossy();
        let agent_socket = files.agent_socket().to_string_lossy();
        let agent_folder = files.agent_folder().to_string_lossy();

        let mut command = Command::new("qemu-system-aarch64");
        command
            .args(["-run-with", "exit-with-parent=on"])
            .args(["-nographic"])
            .args(["-machine", "virt,gic-version=3"])
            .args(["-cpu", "host"])
            .args(["-accel", "hvf"])
            .args(["-smp", "2"])
            .args(["-m", "2048"])
            .args(["-bios", u_boot_bin.as_ref()])
            .args(["-device", "virtio-rng-pci"])
            .args(["-drive", format!("if=virtio,file={wic_qcow2},format=qcow2").as_str()])
            // network 1
            .args(["-netdev", "user,id=lan0"])
            .args(["-device", "virtio-net-pci,netdev=lan0,mac=52:54:00:00:00:10"])
            // network 2
            .args(["-netdev", "user,id=wan"])
            .args(["-device", "virtio-net-pci,netdev=wan,mac=52:54:00:00:00:60"])
            // network 3
            .args(["-netdev", "user,id=wan2"])
            .args(["-device", "virtio-net-pci,netdev=wan2,mac=52:54:00:00:00:80"])
            // the command channel: one port, named so the guest finds it under /dev/virtio-ports
            .args(["-device", "virtio-serial-pci,id=vioserial0"])
            .args([
                "-chardev",
                format!("socket,id=agent0,path={agent_socket},server=on,wait=off").as_str(),
            ])
            .args([
                "-device",
                format!("virtserialport,chardev=agent0,name={AGENT_PORT}").as_str(),
            ])
            // the agent binaries, read-only: the guest mounts this and copies one out
            .args([
                "-fsdev",
                format!("local,id=agentfs,path={agent_folder},security_model=none,readonly=on").as_str(),
            ])
            .args([
                "-device",
                format!("virtio-9p-pci,fsdev=agentfs,mount_tag={AGENT_MOUNT_TAG}").as_str(),
            ])
            // -nographic puts the serial console on stdio, it must never be inherited
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            let message = format!("failed to start qemu-system-aarch64: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| {
            let message = "failed to open QEMU stdin";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            let message = "failed to open QEMU stdout";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        let stderr = child.stderr.take().ok_or_else(|| {
            let message = "failed to open QEMU stderr";
            error!("{message}");
            ErrorData::internal_error(message, None)
        })?;

        info!("QEMU started, pid {:?}", child.id());
        let console = RdkQemuConsoleBuffer::new();
        let (console_tx, console_rx) = tokio::sync::mpsc::channel(64);

        let mut pumps = JoinSet::new();
        pumps.spawn(pump_console(stdout, console.clone(), console_tx.clone()));
        pumps.spawn(pump_console(stderr, console.clone(), console_tx));

        let agent = Arc::new(
            Self::wait_for_agent(request, &mut child, &mut stdin, &files, &console, console_rx, timeout).await?,
        );

        Ok(Self {
            console,
            child,
            _stdin: stdin,
            agent,
            pumps,
            _files: files,
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn shutdown(mut self) -> String {
        if let Ok(Some(status)) = self.child.try_wait() {
            return format!("QEMU had already exited on its own: {status}");
        }

        if let Some(status) = self.power_off().await {
            return format!("the guest powered itself off, QEMU exited: {status}");
        }

        // WARNING: the log line below is only true when a SIGTERM actually went out, which is
        // why both it and the wait for it stay inside the branch that sent one
        if self.terminate() {
            if let Some(status) = self.wait_for_exit(SIGTERM_TIMEOUT).await {
                return format!("QEMU exited on SIGTERM: {status}");
            }
            warn!("QEMU ignored SIGTERM, killing it");
        }

        match self.child.kill().await {
            Ok(()) => "QEMU killed with SIGKILL".to_string(),
            Err(e) => format!("failed to kill QEMU: {e}"),
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn power_off(&mut self) -> Option<ExitStatus> {
        // WARNING: a wedged agent cannot power anything off, and without this probe the minute
        // below would be spent on every stop of a guest that had already lost its agent
        if !self.agent.ping(AGENT_LIVENESS_TIMEOUT).await {
            warn!("the guest agent is not answering, not asking it to power off");
            return None;
        }

        if let Err(e) = self.agent.power_off().await {
            warn!("failed to ask the guest to power off: {e:?}");
            return None;
        }

        let status = self.wait_for_exit(GUEST_POWER_OFF_TIMEOUT).await;
        if status.is_none() {
            let seconds = GUEST_POWER_OFF_TIMEOUT.as_secs();
            warn!("the guest did not power off within {seconds}s, terminating QEMU");
        }

        status
    }

    ////////////////////////////////////////////////////////////////////////////////
    fn terminate(&mut self) -> bool {
        // WARNING: `id` is None once the child has been reaped, which is what keeps this from
        // signalling a pid the system has already handed to somebody else
        let Some(pid) = self.child.id() else {
            return false;
        };

        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            warn!("failed to send SIGTERM to QEMU pid {pid}: {error}");
            return false;
        }

        true
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn wait_for_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(e)) => {
                warn!("failed to wait for QEMU: {e}");
                None
            }
            Err(_) => None,
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn agent(&self) -> Arc<RdkQemuAgent> {
        self.agent.clone()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn wait_for_agent(
        request: &RequestContext<RoleServer>,
        child: &mut Child,
        stdin: &mut ChildStdin,
        files: &RdkQemuFiles,
        console: &RdkQemuConsoleBuffer,
        console_rx: tokio::sync::mpsc::Receiver<String>,
        timeout: Duration,
    ) -> Result<RdkQemuAgent, ErrorData> {
        let progress_token = request.meta.get_progress_token();
        let mut progress = 0.0;
        let mut progress_interval = tokio::time::interval(Duration::from_secs(1));

        let mut deadline_timeout = pin!(tokio::time::sleep(timeout));
        let mut prepare_agent_task = pin!(Self::prepare_agent(stdin, files, console_rx));

        loop {
            tokio::select! {
                agent = &mut prepare_agent_task => {
                    return agent.map_err(|e| {
                        let console = console.dump();
                        let message = format!("QEMU stdout died, reason: {e}, last messages:\n\n{console}");
                        error!("{message}");
                        ErrorData::internal_error(message, None)
                    })
                }

                _ = progress_interval.tick() => {
                    if let Some(token) = progress_token.clone() &&
                        let Some(line) = console.last_line()
                    {
                        progress += 1.0;
                        let param = ProgressNotificationParam::new(token, progress).with_message(line);
                        let _ = request.peer.notify_progress(param).await;
                    }
                }

                status = child.wait() => {
                    let status = match status {
                        Ok(e) => e.to_string(),
                        Err(e) => format!("unknown status: {e}"),
                    };
                    let console = console.dump();
                    let message = format!("QEMU exited during boot: {status}, last messages:\n\n{console}");
                    error!("{message}");
                    return Err(ErrorData::internal_error(message, None));
                }

                _ = &mut deadline_timeout => {
                    let console = console.dump();
                    let message = format!("guest agent not answered, last messages:\n\n{console}");
                    error!("{message}");
                    return Err(ErrorData::internal_error(message, None));
                }
            }
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn prepare_agent(
        stdin: &mut ChildStdin,
        files: &RdkQemuFiles,
        console_rx: tokio::sync::mpsc::Receiver<String>,
    ) -> anyhow::Result<RdkQemuAgent> {
        debug!("waiting for prompt");
        Self::wait_for_prompt(console_rx)
            .await
            .map_err(|e| anyhow!("wait_for_prompt failed: {e}"))?;

        debug!("installing agent");
        Self::install_agent(stdin)
            .await
            .map_err(|e| anyhow!("install_agent failed: {e}"))?;

        debug!("connecting agent");
        Ok(Self::connect_agent(files).await)
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn wait_for_prompt(mut console_rx: tokio::sync::mpsc::Receiver<String>) -> anyhow::Result<()> {
        loop {
            let Some(line) = console_rx.recv().await else {
                bail!("QEMU stdout died");
            };
            if line.contains(CONSOLE_PROMPT_READY) {
                break;
            }
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn install_agent(stdin: &mut ChildStdin) -> anyhow::Result<()> {
        // the prompt means bash printed PS1, not that it is reading yet, and anything typed too
        // early is simply lost with nothing to show for it
        tokio::time::sleep(Duration::from_secs(1)).await;

        for line in console_install_agent() {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n\n").await?;
            stdin.flush().await?;

            // WARNING: one line at a time. The guest tty is in canonical mode,
            // so write longer than its line buffer would be truncated rather than queued.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    async fn connect_agent(files: &RdkQemuFiles) -> RdkQemuAgent {
        let mut interval = tokio::time::interval(AGENT_PROBE_INTERVAL);
        loop {
            interval.tick().await;

            let agent = match RdkQemuAgent::connect(files.agent_socket()).await {
                Ok(e) => e,
                Err(e) => {
                    warn!("failed to connect to agent: {e}");
                    continue;
                }
            };

            if let Err(e) = agent.initialize(AGENT_PROBE_TIMEOUT).await {
                warn!("agent answered but did not initialize: {e}");
                continue;
            }

            return agent;
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn exit_status(&mut self) -> Result<Option<ExitStatus>, ErrorData> {
        self.child.try_wait().map_err(|e| {
            let message = format!("failed to query QEMU state: {e}");
            error!("{message}");
            ErrorData::internal_error(message, None)
        })
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub fn console_dump(&self) -> String {
        self.console.dump()
    }

    ////////////////////////////////////////////////////////////////////////////////
    pub async fn console_dump_after_exit(&mut self) -> String {
        let drain = async { while self.pumps.join_next().await.is_some() {} };
        let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
        self.console_dump()
    }
}

////////////////////////////////////////////////////////////////////////////////
fn console_install_agent() -> Vec<String> {
    const MOUNT: &str = "/tmp/.rdk-agent-9p";
    const BINARY: &str = "/tmp/.rdk-agent";
    const LOG: &str = "/tmp/.rdk-agent.log";

    vec![
        String::new(),
        "set +H 2>/dev/null".to_string(),
        "modprobe 9pnet_virtio 2>/dev/null; modprobe 9p 2>/dev/null".to_string(),
        format!("mkdir -p {MOUNT}"),
        format!("mount -t 9p -o trans=virtio,version=9p2000.L,ro {AGENT_MOUNT_TAG} {MOUNT}"),
        format!("cp {MOUNT}/{AGENT_BINARY} {BINARY} && chmod +x {BINARY}"),
        format!("umount {MOUNT}"),
        format!("setsid {BINARY} /dev/virtio-ports/{AGENT_PORT} </dev/null >{LOG} 2>&1 &"),
        format!("sleep 1; ls -l {BINARY} /dev/virtio-ports/; cat {LOG}"),
    ]
}

////////////////////////////////////////////////////////////////////////////////
async fn pump_console(
    reader: impl AsyncRead + Unpin,
    console: RdkQemuConsoleBuffer,
    output_tx: tokio::sync::mpsc::Sender<String>,
) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();

    loop {
        buffer.clear();

        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let line = String::from_utf8_lossy(buffer.trim_ascii_end());
        let line = line.into_owned();

        if let Ok(permit) = output_tx.reserve().await {
            permit.send(line.clone());
        }

        console.push(line);
    }
}
