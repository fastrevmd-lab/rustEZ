//! Junos device connection and lifecycle management.

use std::time::Duration;

use rustnetconf::Client;

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
        }
    }

    /// Get cached facts, or gather them if not yet cached.
    pub async fn facts(&mut self) -> Result<&Facts, RustEzError> {
        if self.facts_cache.is_none() {
            self.facts_refresh().await?;
        }
        Ok(self.facts_cache.as_ref().unwrap())
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
    #[allow(clippy::result_large_err)]
    pub fn rpc(&mut self) -> Result<RpcExecutor<'_>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        Ok(RpcExecutor::new(client, self.rpc_timeout))
    }

    /// Get a config manager for configuration operations.
    #[allow(clippy::result_large_err)]
    pub fn config(&mut self) -> Result<ConfigManager<'_>, RustEzError> {
        let client = self.client.as_mut().ok_or(RustEzError::NotConnected)?;
        Ok(ConfigManager::new(client, self.rpc_timeout))
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

        let mut client = builder.connect().await?;
        let rpc_timeout = self.rpc_timeout.unwrap_or(DEFAULT_RPC_TIMEOUT);

        let facts_cache = if self.gather_facts {
            Some(facts::gather_facts(&mut client, rpc_timeout).await?)
        } else {
            None
        };

        Ok(Device {
            client: Some(client),
            facts_cache,
            rpc_timeout,
        })
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
        };

        assert!(matches!(
            device.cli("show version").await,
            Err(RustEzError::NotConnected)
        ));
        assert!(matches!(device.rpc(), Err(RustEzError::NotConnected)));
        assert!(matches!(device.config(), Err(RustEzError::NotConnected)));
    }
}
