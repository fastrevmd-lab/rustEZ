//! CLI error taxonomy, command phases, and the RustEzError classifier.

use rustez::RustEzError;

/// Failure category. The discriminant maps directly to the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Usage,
    Connect,
    Auth,
    Rpc,
    Load,
    Commit,
    Rollback,
    Internal,
}

impl ErrorKind {
    /// Process exit code for this error category (0 is reserved for success).
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Usage => 1,
            ErrorKind::Connect => 2,
            ErrorKind::Auth => 3,
            ErrorKind::Rpc => 4,
            ErrorKind::Load => 5,
            ErrorKind::Commit => 6,
            ErrorKind::Rollback => 7,
            ErrorKind::Internal => 8,
        }
    }

    /// Stable lowercase string used in the JSON envelope `error.kind` field.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => "usage",
            ErrorKind::Connect => "connect",
            ErrorKind::Auth => "auth",
            ErrorKind::Rpc => "rpc",
            ErrorKind::Load => "load",
            ErrorKind::Commit => "commit",
            ErrorKind::Rollback => "rollback",
            ErrorKind::Internal => "internal",
        }
    }
}

/// The command phase a library call ran in, so the classifier can map an
/// otherwise-ambiguous device error (e.g. a generic RPC server error) to the
/// right category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Facts,
    Rpc,
    Load,
    Commit,
    Rollback,
}

/// A CLI-level error carrying its category and a human message.
#[derive(Debug)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CliError {
    /// Construct a CLI error with an explicit category.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CliError {
            kind,
            message: message.into(),
        }
    }

    /// Classify a library error in the context of the phase that produced it.
    pub fn from_rustez(err: &RustEzError, phase: Phase) -> Self {
        CliError {
            kind: classify(err, phase),
            message: err.to_string(),
        }
    }
}

/// Default category for an error that is only distinguishable by phase.
fn kind_for_phase(phase: Phase) -> ErrorKind {
    match phase {
        Phase::Connect => ErrorKind::Connect,
        Phase::Facts | Phase::Rpc => ErrorKind::Rpc,
        Phase::Load => ErrorKind::Load,
        Phase::Commit => ErrorKind::Commit,
        Phase::Rollback => ErrorKind::Rollback,
    }
}

/// Map a `RustEzError` to an `ErrorKind`, using `phase` as a tiebreaker.
pub fn classify(err: &RustEzError, phase: Phase) -> ErrorKind {
    use rustnetconf::error::{NetconfError, ProtocolError, RpcError, TransportError};

    match err {
        RustEzError::Netconf(NetconfError::Transport(t)) => match t {
            TransportError::Auth(_) => ErrorKind::Auth,
            _ => ErrorKind::Connect,
        },
        RustEzError::Netconf(NetconfError::Framing(_)) => ErrorKind::Connect,
        RustEzError::Netconf(NetconfError::Protocol(p)) => match p {
            ProtocolError::SessionClosed
            | ProtocolError::SessionExpired
            | ProtocolError::HelloFailed(_) => ErrorKind::Connect,
            _ => ErrorKind::Internal,
        },
        RustEzError::Netconf(NetconfError::Rpc(r)) => match r {
            RpcError::CommitUnknown => ErrorKind::Commit,
            RpcError::ParseError(_) | RpcError::MessageIdMismatch { .. } => ErrorKind::Internal,
            _ => kind_for_phase(phase),
        },
        RustEzError::Timeout(_) => kind_for_phase(phase),
        RustEzError::Config(_) => kind_for_phase(phase),
        RustEzError::Rpc(_) | RustEzError::Facts(_) => ErrorKind::Rpc,
        RustEzError::SshConfig(_) => ErrorKind::Usage,
        RustEzError::NotConnected | RustEzError::XmlParse(_) => ErrorKind::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnetconf::error::{NetconfError, RpcError, RpcServerError, TransportError};

    #[test]
    fn exit_codes_are_distinct_and_match_spec() {
        assert_eq!(ErrorKind::Usage.exit_code(), 1);
        assert_eq!(ErrorKind::Connect.exit_code(), 2);
        assert_eq!(ErrorKind::Auth.exit_code(), 3);
        assert_eq!(ErrorKind::Rpc.exit_code(), 4);
        assert_eq!(ErrorKind::Load.exit_code(), 5);
        assert_eq!(ErrorKind::Commit.exit_code(), 6);
        assert_eq!(ErrorKind::Rollback.exit_code(), 7);
        assert_eq!(ErrorKind::Internal.exit_code(), 8);
    }

    #[test]
    fn auth_transport_error_classifies_as_auth() {
        let err = RustEzError::Netconf(NetconfError::Transport(TransportError::Auth(
            "bad creds".into(),
        )));
        assert_eq!(classify(&err, Phase::Connect), ErrorKind::Auth);
    }

    #[test]
    fn connect_transport_error_classifies_as_connect() {
        let err = RustEzError::Netconf(NetconfError::Transport(TransportError::Connect(
            "refused".into(),
        )));
        assert_eq!(classify(&err, Phase::Connect), ErrorKind::Connect);
    }

    #[test]
    fn host_key_mismatch_classifies_as_connect() {
        let err = RustEzError::Netconf(NetconfError::Transport(TransportError::HostKeyMismatch {
            host: "h".into(),
            expected: "a".into(),
            actual: "b".into(),
        }));
        assert_eq!(classify(&err, Phase::Facts), ErrorKind::Connect);
    }

    #[test]
    fn server_error_uses_phase_during_load() {
        let err = RustEzError::Netconf(NetconfError::Rpc(RpcError::ServerError(Box::new(
            RpcServerError {
                error_type: None,
                tag: rustnetconf::types::ErrorTag::OperationFailed,
                severity: None,
                app_tag: None,
                path: None,
                message: "config rejected".into(),
                info: None,
            },
        ))));
        assert_eq!(classify(&err, Phase::Load), ErrorKind::Load);
        assert_eq!(classify(&err, Phase::Commit), ErrorKind::Commit);
        assert_eq!(classify(&err, Phase::Rollback), ErrorKind::Rollback);
    }

    #[test]
    fn config_error_uses_phase() {
        let err = RustEzError::Config("nope".into());
        assert_eq!(classify(&err, Phase::Load), ErrorKind::Load);
        assert_eq!(classify(&err, Phase::Commit), ErrorKind::Commit);
    }

    #[test]
    fn not_connected_is_internal() {
        assert_eq!(
            classify(&RustEzError::NotConnected, Phase::Rpc),
            ErrorKind::Internal
        );
    }
}
