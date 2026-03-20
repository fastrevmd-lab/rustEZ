//! Error types for rustEZ.

use rustnetconf::error::NetconfError;
use thiserror::Error;

/// Top-level error type for all rustEZ operations.
#[derive(Debug, Error)]
pub enum RustEzError {
    /// Wraps all rustnetconf errors.
    #[error("netconf error: {0}")]
    Netconf(#[from] NetconfError),

    /// Facts gathering failures.
    #[error("facts error: {0}")]
    Facts(String),

    /// Config operation failures.
    #[error("config error: {0}")]
    Config(String),

    /// XML parsing failures.
    #[error("XML parse error: {0}")]
    XmlParse(String),

    /// Operation on a closed device.
    #[error("device is not connected")]
    NotConnected,

    /// RPC-specific errors.
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Per-RPC timeout.
    #[error("timeout: {0}")]
    Timeout(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnetconf::error::{NetconfError, TransportError};

    #[test]
    fn test_from_netconf_error() {
        let netconf_err = NetconfError::Transport(TransportError::Connect("test".to_string()));
        let rustez_err: RustEzError = netconf_err.into();
        assert!(matches!(rustez_err, RustEzError::Netconf(_)));
        assert!(rustez_err.to_string().contains("test"));
    }
}
