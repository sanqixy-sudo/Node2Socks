use crate::ProxyCore;
use crate::controller::MihomoController;
use crate::provider::{ProviderSource, render_provider_topology};
use crate::topology::CoreTopology;
use async_trait::async_trait;
use node2socks_domain::{AppError, AppResult, CoreHealth, CoreState, ErrorCode};
use rand::RngCore;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs, io,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, watch},
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};

pub const MIHOMO_VERSION: &str = "v1.19.30";
pub const MIHOMO_ARCHIVE_SHA256: &str =
    "22c09fd67673895ef7cd6b1820563918275c3d316f2462b306208675118db3c0";
pub const MIHOMO_EXECUTABLE_SHA256: &str =
    "f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct MihomoConfig {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
    pub topology: Option<CoreTopology>,
    pub providers: Vec<ProviderSource>,
    pub socks_port: Option<u16>,
    pub startup_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_restart_attempts: u32,
    pub outbound_interface: Option<String>,
}

impl MihomoConfig {
    pub fn new(executable: impl Into<PathBuf>, runtime_dir: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            runtime_dir: runtime_dir.into(),
            socks_port: None,
            startup_timeout: Duration::from_secs(15),
            shutdown_timeout: Duration::from_secs(5),
            topology: None,
            providers: Vec::new(),
            max_restart_attempts: 3,
            outbound_interface: None,
        }
    }
}

struct RunningCore {
    child: Child,
    controller_port: u16,
    socks_port: u16,
    secret: String,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
}

#[derive(Default)]
struct ManagerState {
    running: Option<RunningCore>,
    restart_attempts: u32,
}

pub struct MihomoManager {
    config: MihomoConfig,
    state: Mutex<ManagerState>,
    logs: Arc<Mutex<VecDeque<String>>>,
    client: Client,
}

pub struct CrashMonitor {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl CrashMonitor {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

impl MihomoManager {
    pub fn new(config: MihomoConfig) -> AppResult<Self> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
        Ok(Self {
            config,
            state: Mutex::new(ManagerState::default()),
            logs: Arc::new(Mutex::new(VecDeque::with_capacity(200))),
            client,
        })
    }

    pub fn verify_binary(&self) -> AppResult<String> {
        if !self.config.executable.is_file() {
            return Err(AppError::new(
                ErrorCode::CoreBinaryMissing,
                format!(
                    "Mihomo executable not found: {}",
                    self.config.executable.display()
                ),
            ));
        }
        let bytes = fs::read(&self.config.executable).map_err(io_error)?;
        let actual = hex::encode(Sha256::digest(bytes));
        if actual != MIHOMO_EXECUTABLE_SHA256 {
            return Err(AppError::new(
                ErrorCode::CoreChecksumFailed,
                format!(
                    "Mihomo executable checksum mismatch: expected {MIHOMO_EXECUTABLE_SHA256}, got {actual}"
                ),
            ));
        }
        Ok(actual)
    }

    pub async fn recent_logs(&self) -> Vec<String> {
        self.logs.lock().await.iter().cloned().collect()
    }

    pub async fn socks_port(&self) -> Option<u16> {
        self.state
            .lock()
            .await
            .running
            .as_ref()
            .map(|core| core.socks_port)
    }

    /// Run bounded crash detection and recovery until explicitly shut down.
    pub fn spawn_crash_monitor(self: Arc<Self>, poll_interval: Duration) -> CrashMonitor {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = shutdown_rx.changed() => {
                        if result.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = sleep(poll_interval) => {
                        match self.recover_if_crashed().await {
                            Ok(Some(health)) => tracing::info!(
                                pid = health.pid,
                                "Mihomo recovered after an unexpected exit"
                            ),
                            Ok(None) => {}
                            Err(error) => tracing::error!(
                                code = %error.code,
                                message = %error.message,
                                "Mihomo crash recovery failed"
                            ),
                        }
                    }
                }
            }
        });
        CrashMonitor { shutdown, task }
    }

    /// Detect a terminated child and recover with bounded exponential backoff.
    pub async fn recover_if_crashed(&self) -> AppResult<Option<CoreHealth>> {
        let crashed = {
            let mut state = self.state.lock().await;
            match state.running.as_mut() {
                Some(core) => match core.child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::warn!(?status, "Mihomo exited unexpectedly");
                        if let Some(core) = state.running.take() {
                            core.stdout_task.abort();
                            core.stderr_task.abort();
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(error) => return Err(io_error(error)),
                },
                None => false,
            }
        };

        if !crashed {
            return Ok(None);
        }

        let attempt = {
            let mut state = self.state.lock().await;
            state.restart_attempts += 1;
            state.restart_attempts
        };
        if attempt > self.config.max_restart_attempts {
            return Err(AppError::new(
                ErrorCode::CoreStartFailed,
                "Mihomo crash restart limit exceeded",
            ));
        }
        let backoff = Duration::from_millis(250 * 2_u64.pow(attempt - 1));
        sleep(backoff).await;
        self.spawn().await.map(Some)
    }

    async fn spawn(&self) -> AppResult<CoreHealth> {
        self.verify_binary()?;
        fs::create_dir_all(&self.config.runtime_dir).map_err(io_error)?;

        let controller_port = reserve_local_port()?;
        let socks_port = match self.config.socks_port {
            Some(port) => ensure_local_port_available(port)?,
            None => reserve_local_port()?,
        };
        if controller_port == socks_port {
            return Err(AppError::new(
                ErrorCode::PortInUse,
                "controller and SOCKS listener selected the same port",
            ));
        }
        let secret = random_secret();
        let config_path = self.config.runtime_dir.join("config.yaml");
        let mut runtime_config = match &self.config.topology {
            Some(topology) => render_provider_topology(
                topology,
                &self.config.providers,
                controller_port,
                &secret,
            )?,
            None => render_config(controller_port, socks_port, &secret),
        };
        if let Some(interface) = &self.config.outbound_interface {
            if interface.contains(['\r', '\n']) {
                return Err(AppError::new(
                    ErrorCode::InvalidConfiguration,
                    "outbound interface contains a newline",
                ));
            }
            runtime_config = format!(
                "interface-name: \"{}\"\n{runtime_config}",
                interface.replace('\\', "\\\\").replace('"', "\\\"")
            );
        }
        fs::write(&config_path, runtime_config).map_err(io_error)?;

        let mut command = Command::new(&self.config.executable);

        command
            .arg("-d")
            .arg(&self.config.runtime_dir)
            .arg("-f")
            .arg(&config_path)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command.spawn().map_err(|error| {
            AppError::new(ErrorCode::CoreStartFailed, format!("spawn Mihomo: {error}"))
        })?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::new(ErrorCode::CoreStartFailed, "Mihomo stdout pipe unavailable")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AppError::new(ErrorCode::CoreStartFailed, "Mihomo stderr pipe unavailable")
        })?;
        let mut log_secrets = vec![secret.clone()];
        log_secrets.extend(
            self.config
                .providers
                .iter()
                .map(|provider| provider.bearer_token.clone()),
        );
        let stdout_task = capture_logs(stdout, self.logs.clone(), log_secrets.clone(), false);
        let stderr_task = capture_logs(stderr, self.logs.clone(), log_secrets, true);
        self.state.lock().await.running = Some(RunningCore {
            child,
            controller_port,
            socks_port,
            secret: secret.clone(),
            stdout_task,
            stderr_task,
        });

        match self.wait_until_healthy(self.config.startup_timeout).await {
            Ok(health) => Ok(CoreHealth { pid, ..health }),
            Err(error) => {
                let _ = self.stop().await;
                Err(error)
            }
        }
    }
    pub async fn controller(&self) -> AppResult<MihomoController> {
        let (_, port, secret) = self.controller_snapshot().await?;
        MihomoController::new(port, secret)
    }

    async fn wait_until_healthy(&self, duration: Duration) -> AppResult<CoreHealth> {
        let deadline = Instant::now() + duration;
        let mut last_error = String::from("controller did not respond");
        while Instant::now() < deadline {
            match self.health().await {
                Ok(health) if health.state == CoreState::Running => return Ok(health),
                Ok(_) => {}
                Err(error) => last_error = error.to_string(),
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err(AppError::new(
            ErrorCode::CoreUnhealthy,
            format!("Mihomo controller was not healthy before timeout: {last_error}"),
        ))
    }

    async fn controller_snapshot(&self) -> AppResult<(u32, u16, String)> {
        let mut state = self.state.lock().await;
        let core = state
            .running
            .as_mut()
            .ok_or_else(|| AppError::new(ErrorCode::CoreNotRunning, "Mihomo is not running"))?;
        if let Some(status) = core.child.try_wait().map_err(io_error)? {
            return Err(AppError::new(
                ErrorCode::CoreUnhealthy,
                format!("Mihomo exited with {status}"),
            ));
        }
        Ok((
            core.child.id().unwrap_or_default(),
            core.controller_port,
            core.secret.clone(),
        ))
    }
}

#[async_trait]
impl ProxyCore for MihomoManager {
    async fn start(&self) -> AppResult<CoreHealth> {
        if self.state.lock().await.running.is_some() {
            return self.health().await;
        }
        self.state.lock().await.restart_attempts = 0;
        self.spawn().await
    }

    async fn stop(&self) -> AppResult<()> {
        let running = self.state.lock().await.running.take();
        let Some(mut core) = running else {
            return Ok(());
        };
        core.child.start_kill().map_err(|error| {
            AppError::new(
                ErrorCode::CoreShutdownFailed,
                format!("terminate Mihomo: {error}"),
            )
        })?;
        timeout(self.config.shutdown_timeout, core.child.wait())
            .await
            .map_err(|_| {
                AppError::new(
                    ErrorCode::CoreShutdownFailed,
                    "timed out waiting for Mihomo to stop",
                )
            })?
            .map_err(io_error)?;
        core.stdout_task.abort();
        core.stderr_task.abort();
        let config_path = self.config.runtime_dir.join("config.yaml");
        match fs::remove_file(config_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        Ok(())
    }

    async fn restart(&self) -> AppResult<CoreHealth> {
        self.stop().await?;
        self.start().await
    }

    async fn health(&self) -> AppResult<CoreHealth> {
        let (pid, controller_port, secret) = self.controller_snapshot().await?;
        let url = format!("http://127.0.0.1:{controller_port}/version");
        let response = self
            .client
            .get(&url)
            .bearer_auth(secret)
            .send()
            .await
            .map_err(|error| AppError::new(ErrorCode::CoreUnhealthy, error.to_string()))?;
        if response.status() != StatusCode::OK {
            return Err(AppError::new(
                ErrorCode::CoreUnhealthy,
                format!("controller returned HTTP {}", response.status()),
            ));
        }
        #[derive(Deserialize)]
        struct VersionResponse {
            version: Option<String>,
        }
        let version = response
            .json::<VersionResponse>()
            .await
            .map_err(|error| AppError::new(ErrorCode::CoreUnhealthy, error.to_string()))?
            .version;
        Ok(CoreHealth {
            state: CoreState::Running,
            pid: Some(pid),
            controller_address: Some(format!("127.0.0.1:{controller_port}")),
            version,
        })
    }
}

fn reserve_local_port() -> AppResult<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(io_error)
}

fn ensure_local_port_available(port: u16) -> AppResult<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .map(|_| port)
        .map_err(|error| {
            AppError::new(
                ErrorCode::PortInUse,
                format!("127.0.0.1:{port} is not available: {error}"),
            )
        })
}

fn random_secret() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn render_config(controller_port: u16, socks_port: u16, secret: &str) -> String {
    format!(
        concat!(
            "allow-lan: false\n",
            "bind-address: 127.0.0.1\n",
            "mode: rule\n",
            "log-level: info\n",
            "ipv6: false\n",
            "external-controller: 127.0.0.1:{controller_port}\n",
            "secret: \"{secret}\"\n",
            "external-controller-cors:\n",
            "  allow-private-network: false\n",
            "listeners:\n",
            "  - name: slot-poc-in\n",
            "    type: socks\n",
            "    listen: 127.0.0.1\n",
            "    port: {socks_port}\n",
            "    proxy: REJECT\n",
            "    udp: false\n",
            "proxies: []\n",
            "proxy-groups: []\n",
            "rules: []\n",
        ),
        controller_port = controller_port,
        secret = secret,
        socks_port = socks_port
    )
}

fn capture_logs<R>(
    stream: R,
    logs: Arc<Mutex<VecDeque<String>>>,
    secrets: Vec<String>,
    is_error: bool,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let redacted = redact_log_line(&line, &secrets);
            if is_error {
                tracing::warn!(target: "mihomo", "{redacted}");
            } else {
                tracing::info!(target: "mihomo", "{redacted}");
            }
            let mut buffer = logs.lock().await;
            if buffer.len() == 200 {
                buffer.pop_front();
            }
            buffer.push_back(redacted);
        }
    })
}

fn redact_log_line(line: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(line.to_owned(), |value, secret| {
            value.replace(secret, "[REDACTED]")
        })
}

fn io_error(error: io::Error) -> AppError {
    AppError::new(ErrorCode::IoError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_log_redaction_covers_controller_and_provider_secrets() {
        let line = "controller=controller-secret authorization=provider-secret";
        let redacted = redact_log_line(
            line,
            &["controller-secret".into(), "provider-secret".into()],
        );
        assert_eq!(redacted, "controller=[REDACTED] authorization=[REDACTED]");
    }

    #[test]
    fn runtime_config_is_localhost_only_and_rejects_traffic() {
        let config = render_config(19_090, 21_001, "secret-value");
        assert!(config.contains("external-controller: 127.0.0.1:19090"));
        assert!(config.contains("listen: 127.0.0.1"));
        assert!(config.contains("proxy: REJECT"));
        assert!(!config.contains("0.0.0.0"));
    }

    #[test]
    fn generated_secrets_are_random_and_256_bit() {
        let first = random_secret();
        let second = random_secret();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }

    #[test]
    fn fixed_release_metadata_is_present() {
        assert_eq!(MIHOMO_VERSION, "v1.19.30");
        assert_eq!(MIHOMO_ARCHIVE_SHA256.len(), 64);
        assert_eq!(MIHOMO_EXECUTABLE_SHA256.len(), 64);
    }
}
