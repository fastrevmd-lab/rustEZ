//! Junos device connection and lifecycle management.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rustnetconf::transport::ssh::{HostKeyVerification, JumpHostConfig};
use rustnetconf::{Client, Notification, SshConfigError, SshConfigFile};

use crate::config::ConfigManager;
use crate::error::RustEzError;
use crate::facts::{self, Facts};
use crate::rpc::RpcExecutor;

/// Default per-RPC timeout.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// A connected Junos device.
///
/// Created via [`Device::connect()`] which returns a [`DeviceBuilder`].
///
/// ```rust,no_run
/// use rustez::Device;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut dev = Device::connect("10.0.0.1")
///     .username("admin")
///     .password("secret")
///     .open()
///     .await?;
///
/// let facts = dev.facts().await?;
/// println!("{} running {}", facts.hostname, facts.version);
///
/// dev.close().await?;
/// # Ok(())
/// # }
/// ```
pub struct Device {
    client: Option<Client>,
    facts_cache: Option<Facts>,
    rpc_timeout: Duration,
    config_db_open: bool,
}

impl Device {
    /// Start building a connection to a Junos device.
    ///
    /// Returns a [`DeviceBuilder`] for configuring credentials and options.
    pub fn connect(host: &str) -> DeviceBuilder {
        DeviceBuilder {
            host: host.to_string(),
            port: None,
            username: None,
            password: None,
            key_file: None,
            gather_facts: true,
            rpc_timeout: None,
            keepalive_interval: None,
            jump_hosts: Vec::new(),
            proxy_command: None,
            host_key_verification: None,
        }
    }

    /// Build a connection using settings from the user's default SSH config
    /// (`$HOME/.ssh/config`).
    ///
    /// Settings derived from the config:
    ///
    /// - `HostName` → connect target (falls back to `alias` if unset)
    /// - `Port` → port (falls back to NETCONF default 830)
    /// - `User` → [`DeviceBuilder::username`]
    /// - `IdentityFile` → [`DeviceBuilder::key_file`]
    /// - `ProxyJump` → [`DeviceBuilder::jump_hosts`]
    /// - `ProxyCommand` → [`DeviceBuilder::proxy_command`]
    ///
    /// Subsequent builder calls (e.g. `.username()`, `.password()`) override
    /// what the config provided.
    ///
    /// ```rust,no_run
    /// use rustez::Device;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // ~/.ssh/config has `Host edge-r1` block with HostName/User/ProxyJump.
    /// let dev = Device::connect_via_ssh_config("edge-r1")?
    ///     .password("secret")
    ///     .open()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect_via_ssh_config(alias: &str) -> Result<DeviceBuilder, SshConfigError> {
        let path = default_ssh_config_path().ok_or_else(|| SshConfigError::Io {
            path: PathBuf::from("~/.ssh/config"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "$HOME is not set; cannot locate default ssh config",
            ),
        })?;
        Self::connect_via_ssh_config_at(&path, alias)
    }

    /// Like [`Self::connect_via_ssh_config`] but reads from the explicit
    /// `path` instead of `$HOME/.ssh/config`.
    pub fn connect_via_ssh_config_at(
        path: &Path,
        alias: &str,
    ) -> Result<DeviceBuilder, SshConfigError> {
        let cfg = SshConfigFile::load(path)?;
        let resolved = cfg.resolve(alias);

        let host = resolved.hostname.unwrap_or_else(|| alias.to_string());
        // NETCONF default is 830, not 22.
        let port = resolved.port.unwrap_or(830);

        Ok(DeviceBuilder {
            host,
            port: Some(port),
            username: resolved.user,
            password: None,
            key_file: resolved.identity_file,
            gather_facts: true,
            rpc_timeout: None,
            keepalive_interval: None,
            jump_hosts: resolved.jump_hosts,
            proxy_command: resolved.proxy_command,
            host_key_verification: None,
        })
    }

    /// Get cached facts, or gather them if not yet cached.
    pub async fn facts(&mut self) -> Result<&Facts, RustEzError> {
        if self.facts_cache.is_none() {
            self.facts_refresh().await?;
        }
        Ok(self.facts_cache.as_ref().unwrap())
    }

    /// Manually set cached facts, replacing any existing values.
    ///
    /// Useful after connecting with `.no_facts()` to populate facts
    /// without sending RPCs (e.g., clustered SRX with unreachable peer).
    pub fn set_facts(&mut self, facts: Facts) {
        self.facts_cache = Some(facts);
    }

    /// Force re-gather facts from the device.
    pub async fn facts_refresh(&mut self) -> Result<&Facts, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let new_facts = facts::gather_facts(client, self.rpc_timeout).await?;
        self.facts_cache = Some(new_facts);
        Ok(self.facts_cache.as_ref().unwrap())
    }

    /// Execute a CLI command on the device.
    ///
    /// Equivalent to running a command in the Junos CLI.
    /// Returns the text output.
    pub async fn cli(&mut self, command: &str) -> Result<String, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let mut executor = RpcExecutor::new(client, self.rpc_timeout);
        executor.cli(command, "text").await
    }

    /// Get an RPC executor for sending arbitrary RPCs.
    pub fn rpc(&mut self) -> Result<RpcExecutor<'_>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        Ok(RpcExecutor::new(client, self.rpc_timeout))
    }

    /// Get direct mutable access to the underlying rustnetconf `Client`.
    ///
    /// Use this for operations that need native client methods without
    /// going through `RpcExecutor` or `ConfigManager`.
    pub fn client_mut(&mut self) -> Result<&mut Client, RustEzError> {
        self.client.as_mut().ok_or(RustEzError::NotConnected)
    }

    /// Get a config manager for configuration operations.
    ///
    /// On chassis-clustered devices, the config manager will automatically
    /// open a private configuration database before loading config and
    /// close it on unlock. Use [`open_configuration()`](Self::open_configuration)
    /// for explicit control (e.g., exclusive mode).
    pub fn config(&mut self) -> Result<ConfigManager<'_>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        Ok(ConfigManager::new(
            client,
            self.rpc_timeout,
            &mut self.config_db_open,
        ))
    }

    /// Open a private or exclusive configuration database (Junos).
    ///
    /// Only needed on chassis-clustered devices. Call this before
    /// [`config().load()`](ConfigManager::load) if you need exclusive mode.
    /// For private mode, the config manager handles this automatically.
    pub async fn open_configuration(
        &mut self,
        mode: rustnetconf::OpenConfigurationMode,
    ) -> Result<(), RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let timeout = self.rpc_timeout;
        match tokio::time::timeout(timeout, client.open_configuration(mode)).await {
            Ok(inner) => inner?,
            Err(_) => {
                return Err(RustEzError::Timeout(
                    "open_configuration timed out".to_string(),
                ))
            }
        }
        self.config_db_open = true;
        Ok(())
    }

    /// Close a previously opened configuration database (Junos).
    ///
    /// No-op if no configuration database is open.
    pub async fn close_configuration(&mut self) -> Result<(), RustEzError> {
        if !self.config_db_open {
            return Ok(());
        }
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let timeout = self.rpc_timeout;
        match tokio::time::timeout(timeout, client.close_configuration()).await {
            Ok(inner) => inner?,
            Err(_) => {
                return Err(RustEzError::Timeout(
                    "close_configuration timed out".to_string(),
                ))
            }
        }
        self.config_db_open = false;
        Ok(())
    }

    /// Whether the device is part of a chassis cluster.
    ///
    /// Returns `false` if facts have not been gathered yet.
    pub fn is_cluster(&self) -> bool {
        self.facts_cache.as_ref().is_some_and(|f| f.is_cluster)
    }

    /// Check if the NETCONF session is alive (in-memory check, no RPC sent).
    pub fn session_alive(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.session_alive())
    }

    /// Whether a configuration database is currently open (locked).
    pub fn is_config_db_open(&self) -> bool {
        self.config_db_open
    }

    /// Reconnect to the device using the original connection parameters.
    ///
    /// Closes the current session and establishes a fresh SSH/NETCONF connection.
    /// Facts cache is cleared on reconnect.
    pub async fn reconnect(&mut self) -> Result<(), RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        client.reconnect().await?;
        self.facts_cache = None;
        Ok(())
    }

    // ── Notification operations (RFC 5277) ───────────────────────────

    /// Subscribe to device event notifications (RFC 5277).
    ///
    /// Requires the `:notification` capability on the device. After subscribing,
    /// retrieve notifications with [`drain_notifications()`](Self::drain_notifications)
    /// or [`recv_notification()`](Self::recv_notification).
    pub async fn create_subscription(
        &mut self,
        stream: Option<&str>,
        filter: Option<&str>,
        start_time: Option<&str>,
        stop_time: Option<&str>,
    ) -> Result<(), RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let timeout = self.rpc_timeout;
        match tokio::time::timeout(
            timeout,
            client.create_subscription(stream, filter, start_time, stop_time),
        )
        .await
        {
            Ok(inner) => inner?,
            Err(_) => {
                return Err(RustEzError::Timeout(
                    "create_subscription timed out".to_string(),
                ))
            }
        }
        Ok(())
    }

    /// Drain all buffered notifications, returning them and clearing the buffer.
    ///
    /// Notifications are buffered when they arrive during RPC exchanges.
    pub fn drain_notifications(&mut self) -> Result<Vec<Notification>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        Ok(client.drain_notifications())
    }

    /// Wait for the next notification from the device.
    ///
    /// Returns `Ok(None)` if the connection is closed.
    pub async fn recv_notification(&mut self) -> Result<Option<Notification>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        let timeout = self.rpc_timeout;
        match tokio::time::timeout(timeout, client.recv_notification()).await {
            Ok(inner) => Ok(inner?),
            Err(_) => Err(RustEzError::Timeout(
                "recv_notification timed out".to_string(),
            )),
        }
    }

    /// Check if any notifications are buffered without blocking.
    pub fn has_notifications(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.has_notifications())
    }

    /// Whether this session has an active notification subscription.
    pub fn has_subscription(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.has_subscription())
    }

    /// Whether this session may have modified the shared candidate datastore.
    ///
    /// Returns `true` when this session has performed a candidate-modifying
    /// operation — a config load or a rollback — that has not since been
    /// followed by a successful commit or discard. A closed device returns
    /// `false`.
    ///
    /// Note that raw RPCs sent through [`rpc()`](Self::rpc) do **not** set this
    /// on their own; rustnetconf only tracks its typed operations, plus the
    /// explicit mark `load_with_warnings()` performs.
    ///
    /// Useful for connection-pooling layers to decide whether a session is safe
    /// to return to the pool or must be closed to ensure a discard happens.
    pub fn touched_candidate(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.candidate_dirty())
    }

    /// Close the NETCONF session gracefully.
    ///
    /// Idempotent — calling close on an already-closed device is a no-op.
    pub async fn close(&mut self) -> Result<(), RustEzError> {
        if let Some(mut client) = self.client.take() {
            client.close_session().await?;
        }
        Ok(())
    }
}

/// Builder for configuring and opening a [`Device`] connection.
pub struct DeviceBuilder {
    host: String,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    key_file: Option<String>,
    gather_facts: bool,
    rpc_timeout: Option<Duration>,
    keepalive_interval: Option<Duration>,
    jump_hosts: Vec<JumpHostConfig>,
    proxy_command: Option<String>,
    host_key_verification: Option<HostKeyVerification>,
}

impl DeviceBuilder {
    /// Set the SSH username.
    pub fn username(mut self, username: &str) -> Self {
        self.username = Some(username.to_string());
        self
    }

    /// Set the SSH password.
    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.to_string());
        self
    }

    /// Set the SSH private key file path.
    pub fn key_file(mut self, path: &str) -> Self {
        self.key_file = Some(path.to_string());
        self
    }

    /// Set the NETCONF port (default: 830).
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Skip automatic facts gathering on connect.
    pub fn no_facts(mut self) -> Self {
        self.gather_facts = false;
        self
    }

    /// Set the per-RPC timeout (default: 30s).
    pub fn rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = Some(timeout);
        self
    }

    /// Set the keepalive interval for idle session probing.
    ///
    /// When set, the client sends a lightweight probe before RPCs if idle
    /// time exceeds this interval. Disabled by default.
    pub fn keepalive_interval(mut self, interval: Duration) -> Self {
        self.keepalive_interval = Some(interval);
        self
    }

    /// Set an OpenSSH-style `ProxyJump` chain.
    ///
    /// Each hop carries its own credentials and host-key-verification policy.
    /// The hops are dialed in order: hop 0 directly, hop 1 through hop 0's
    /// `direct-tcpip`, etc., and the final target through the last hop.
    ///
    /// Mutually exclusive with [`Self::proxy_command`].
    pub fn jump_hosts(mut self, hops: Vec<JumpHostConfig>) -> Self {
        self.jump_hosts = hops;
        self
    }

    /// Set an OpenSSH-style `ProxyCommand`.
    ///
    /// The command is interpreted by `sh -c` and its stdin/stdout become
    /// the SSH transport stream to the target. The substrings `%h` and
    /// `%p` are replaced with the target host and port respectively.
    ///
    /// Mutually exclusive with [`Self::jump_hosts`].
    pub fn proxy_command(mut self, command: &str) -> Self {
        self.proxy_command = Some(command.to_string());
        self
    }

    /// Set the SSH host-key verification policy for the final target.
    ///
    /// When unset, rustnetconf's default applies — currently
    /// [`HostKeyVerification::RejectAll`] (fail-closed), so connections will
    /// be refused until a policy is chosen. Production callers should pin a
    /// fingerprint with [`HostKeyVerification::Fingerprint`] or supply a
    /// `known_hosts` file with [`HostKeyVerification::KnownHosts`].
    /// Lab/test code may opt in to [`HostKeyVerification::AcceptAll`].
    ///
    /// Obtain a device fingerprint with:
    /// `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` (on the device).
    pub fn host_key_verification(mut self, policy: HostKeyVerification) -> Self {
        self.host_key_verification = Some(policy);
        self
    }

    /// Open the connection to the device.
    ///
    /// Establishes the SSH/NETCONF session and optionally gathers facts.
    pub async fn open(self) -> Result<Device, RustEzError> {
        let address = match self.port {
            Some(port) => format!("{}:{}", self.host, port),
            None => self.host.clone(),
        };

        let mut builder = Client::connect(&address);

        if let Some(ref username) = self.username {
            builder = builder.username(username);
        }
        if let Some(ref password) = self.password {
            builder = builder.password(password);
        }
        if let Some(ref key_file) = self.key_file {
            builder = builder.key_file(key_file);
        }
        if let Some(interval) = self.keepalive_interval {
            builder = builder.keepalive_interval(interval);
        }
        if !self.jump_hosts.is_empty() {
            builder = builder.jump_hosts(self.jump_hosts);
        }
        if let Some(ref command) = self.proxy_command {
            builder = builder.proxy_command(command);
        }
        if let Some(policy) = self.host_key_verification {
            builder = builder.host_key_verification(policy);
        }

        let mut client = builder.connect().await?;
        let rpc_timeout = self.rpc_timeout.unwrap_or(DEFAULT_RPC_TIMEOUT);

        let facts_cache = if self.gather_facts {
            let gathered = facts::gather_facts(&mut client, rpc_timeout).await?;
            log_session_limit_warning(&gathered.personality);
            Some(gathered)
        } else {
            None
        };

        Ok(Device {
            client: Some(client),
            facts_cache,
            rpc_timeout,
            config_db_open: false,
        })
    }
}

/// Locate the user's default SSH config (`$HOME/.ssh/config`).
fn default_ssh_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        let mut p = PathBuf::from(home);
        p.push(".ssh");
        p.push("config");
        p
    })
}

/// Log a warning for platforms with low NETCONF session limits.
fn log_session_limit_warning(personality: &facts::Personality) {
    match personality {
        facts::Personality::Srx | facts::Personality::Vsrx => {
            tracing::warn!(
                platform = %personality,
                max_sessions = 3,
                "this platform limits concurrent NETCONF sessions to 3 — \
                 exceeding this will cause connection resets"
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_close_idempotent() {
        // Device with no client (already closed state)
        let mut device = Device {
            client: None,
            facts_cache: None,
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
            config_db_open: false,
        };

        // First close — no-op, should succeed
        assert!(device.close().await.is_ok());
        // Second close — still a no-op
        assert!(device.close().await.is_ok());
    }

    #[tokio::test]
    async fn test_operations_on_closed_device() {
        let mut device = Device {
            client: None,
            facts_cache: None,
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
            config_db_open: false,
        };

        assert!(matches!(
            device.cli("show version").await,
            Err(RustEzError::NotConnected)
        ));
        assert!(matches!(device.rpc(), Err(RustEzError::NotConnected)));
        assert!(matches!(device.config(), Err(RustEzError::NotConnected)));
    }

    #[test]
    fn test_touched_candidate_on_closed_device() {
        let device = Device {
            client: None,
            facts_cache: None,
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
            config_db_open: false,
        };

        // A closed device has not touched the candidate.
        assert!(!device.touched_candidate());
    }

    #[test]
    fn test_jump_hosts_builder_sets_field() {
        let hops = vec![JumpHostConfig {
            host: "bastion.example.com".to_string(),
            port: 22,
            username: "jump".to_string(),
            auth: rustnetconf::transport::ssh::SshAuth::Agent,
            host_key_verification: rustnetconf::transport::ssh::HostKeyVerification::AcceptAll,
        }];
        let builder = Device::connect("10.0.0.1").jump_hosts(hops);
        assert_eq!(builder.jump_hosts.len(), 1);
        assert_eq!(builder.jump_hosts[0].host, "bastion.example.com");
    }

    #[test]
    fn test_proxy_command_builder_sets_field() {
        let builder = Device::connect("10.0.0.1").proxy_command("ssh -W %h:%p bastion.example.com");
        assert_eq!(
            builder.proxy_command.as_deref(),
            Some("ssh -W %h:%p bastion.example.com")
        );
    }

    #[test]
    fn test_host_key_verification_builder_sets_field() {
        use rustnetconf::transport::ssh::HostKeyVerification;

        let builder = Device::connect("10.0.0.1")
            .host_key_verification(HostKeyVerification::Fingerprint("abc123".to_string()));
        assert!(matches!(
            builder.host_key_verification,
            Some(HostKeyVerification::Fingerprint(ref fp)) if fp == "abc123"
        ));
    }

    #[test]
    fn test_host_key_verification_default_is_none() {
        // Builders that do not call host_key_verification() leave the field unset,
        // deferring to rustnetconf's default (RejectAll, fail-closed) at connect time.
        let builder = Device::connect("10.0.0.1");
        assert!(builder.host_key_verification.is_none());
    }

    #[test]
    fn test_connect_via_ssh_config_at_resolves_fields() {
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        writeln!(
            tmp,
            "Host edge-r1\n  HostName 10.0.0.1\n  Port 8830\n  User netconf\n  IdentityFile /tmp/id\n  ProxyJump bastion.example.com\n"
        )
        .unwrap();

        let builder =
            Device::connect_via_ssh_config_at(tmp.path(), "edge-r1").expect("ssh_config resolves");

        assert_eq!(builder.host, "10.0.0.1");
        assert_eq!(builder.port, Some(8830));
        assert_eq!(builder.username.as_deref(), Some("netconf"));
        assert_eq!(builder.key_file.as_deref(), Some("/tmp/id"));
        assert_eq!(builder.jump_hosts.len(), 1);
        assert_eq!(builder.jump_hosts[0].host, "bastion.example.com");
        assert!(builder.proxy_command.is_none());
    }

    #[test]
    fn test_connect_via_ssh_config_at_uses_alias_when_no_hostname() {
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        // No matching Host block — resolve returns defaults; alias is used
        // as the connect target and port falls back to NETCONF default.
        writeln!(tmp, "Host other\n  HostName 10.0.0.99\n").unwrap();

        let builder =
            Device::connect_via_ssh_config_at(tmp.path(), "edge-r1").expect("ssh_config resolves");

        assert_eq!(builder.host, "edge-r1");
        assert_eq!(builder.port, Some(830));
        assert!(builder.jump_hosts.is_empty());
        assert!(builder.proxy_command.is_none());
    }

    #[test]
    fn test_connect_via_ssh_config_at_missing_file() {
        let result = Device::connect_via_ssh_config_at(Path::new("/nonexistent/ssh/config"), "any");
        assert!(matches!(result, Err(SshConfigError::Io { .. })));
    }

    #[tokio::test]
    async fn test_set_facts_populates_cache() {
        let mut device = Device {
            client: None,
            facts_cache: None,
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
            config_db_open: false,
        };

        assert!(device.facts_cache.is_none());

        let manual_facts = Facts {
            hostname: "vsrx-test1".to_string(),
            model: "vSRX".to_string(),
            version: "21.4R3".to_string(),
            serial_number: "ABC123".to_string(),
            personality: facts::Personality::Vsrx,
            route_engines: vec![],
            master_re: None,
            domain: None,
            fqdn: None,
            is_cluster: false,
        };

        device.set_facts(manual_facts);

        let cached = device.facts_cache.as_ref().unwrap();
        assert_eq!(cached.hostname, "vsrx-test1");
        assert_eq!(cached.model, "vSRX");
        assert_eq!(cached.serial_number, "ABC123");
    }
}
