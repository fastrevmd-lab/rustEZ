//! Device connection setup: credential resolution and host-key policy mapping.

use std::io::IsTerminal;
use std::time::Duration;

use rustez::{Device, HostKeyVerification};

use crate::cli::ConnOpts;
use crate::error::{CliError, ErrorKind, Phase};

/// How the password should be obtained, decided purely from inputs (testable).
#[derive(Debug, PartialEq, Eq)]
pub enum PasswordPlan {
    /// Use this password value directly.
    Use(String),
    /// Prompt interactively (no echo).
    Prompt,
    /// No password needed — key-based auth.
    KeyOnly,
}

/// Decide how to obtain the password. Precedence: flag > env > key > prompt.
///
/// Returns a `usage` error when no source is available and stdin is not a TTY.
pub fn plan_password(
    flag: Option<&str>,
    env: Option<&str>,
    has_key: bool,
    is_tty: bool,
) -> Result<PasswordPlan, CliError> {
    if let Some(p) = flag {
        return Ok(PasswordPlan::Use(p.to_string()));
    }
    if let Some(e) = env {
        return Ok(PasswordPlan::Use(e.to_string()));
    }
    if has_key {
        return Ok(PasswordPlan::KeyOnly);
    }
    if is_tty {
        return Ok(PasswordPlan::Prompt);
    }
    Err(CliError::new(
        ErrorKind::Usage,
        "no password provided and stdin is not a TTY; set $RUSTEZ_PASSWORD or use --key-file",
    ))
}

/// Map host-key CLI flags to a verification policy. `None` => library default (RejectAll).
pub fn host_key_policy(conn: &ConnOpts) -> Option<HostKeyVerification> {
    if let Some(fp) = &conn.host_key_fingerprint {
        return Some(HostKeyVerification::Fingerprint(fp.clone()));
    }
    if let Some(path) = &conn.known_hosts {
        return Some(HostKeyVerification::KnownHosts(path.into()));
    }
    if conn.accept_any_host_key {
        return Some(HostKeyVerification::AcceptAll);
    }
    None
}

/// Build and open a `Device` from connection options.
///
/// `gather_facts` controls whether facts are auto-gathered on open (true for
/// the `facts` command, false for `rpc`/`config` to save three RPCs).
pub async fn build_device(conn: &ConnOpts, gather_facts: bool) -> Result<Device, CliError> {
    if conn.password.is_some() {
        eprintln!(
            "warning: --password is visible in the process list; prefer $RUSTEZ_PASSWORD or --key-file"
        );
    }

    let env_pw = std::env::var("RUSTEZ_PASSWORD").ok();
    let has_key = conn.key_file.is_some();
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_password(conn.password.as_deref(), env_pw.as_deref(), has_key, is_tty)?;

    let password = match plan {
        PasswordPlan::Use(p) => Some(p),
        PasswordPlan::KeyOnly => None,
        PasswordPlan::Prompt => {
            let prompt = format!("Password for {}@{}: ", conn.user, conn.host);
            let pw = rpassword::prompt_password(prompt).map_err(|e| {
                CliError::new(ErrorKind::Usage, format!("failed to read password: {e}"))
            })?;
            Some(pw)
        }
    };

    let mut builder = Device::connect(&conn.host).username(&conn.user);
    if let Some(pw) = &password {
        builder = builder.password(pw);
    }
    if let Some(kf) = &conn.key_file {
        builder = builder.key_file(kf);
    }
    if let Some(port) = conn.port {
        builder = builder.port(port);
    }
    if let Some(secs) = conn.timeout {
        builder = builder.rpc_timeout(Duration::from_secs(secs));
    }
    if let Some(policy) = host_key_policy(conn) {
        builder = builder.host_key_verification(policy);
    }
    if !gather_facts {
        builder = builder.no_facts();
    }

    builder
        .open()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Connect))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_password_takes_precedence() {
        let plan = plan_password(Some("flagpw"), Some("envpw"), false, false).unwrap();
        assert_eq!(plan, PasswordPlan::Use("flagpw".into()));
    }

    #[test]
    fn env_password_used_when_no_flag() {
        let plan = plan_password(None, Some("envpw"), false, false).unwrap();
        assert_eq!(plan, PasswordPlan::Use("envpw".into()));
    }

    #[test]
    fn key_only_when_no_password_source() {
        let plan = plan_password(None, None, true, false).unwrap();
        assert_eq!(plan, PasswordPlan::KeyOnly);
    }

    #[test]
    fn prompt_when_tty_and_no_other_source() {
        let plan = plan_password(None, None, false, true).unwrap();
        assert_eq!(plan, PasswordPlan::Prompt);
    }

    #[test]
    fn usage_error_when_no_source_and_not_tty() {
        let err = plan_password(None, None, false, false).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage);
    }

    #[test]
    fn fingerprint_flag_maps_to_policy() {
        let conn = test_conn(|c| c.host_key_fingerprint = Some("SHA256:x".into()));
        assert!(matches!(
            host_key_policy(&conn),
            Some(HostKeyVerification::Fingerprint(_))
        ));
    }

    #[test]
    fn no_host_key_flag_returns_none() {
        let conn = test_conn(|_| {});
        assert!(host_key_policy(&conn).is_none());
    }

    /// Build a default ConnOpts and let the closure tweak it.
    fn test_conn(tweak: impl FnOnce(&mut ConnOpts)) -> ConnOpts {
        let mut conn = ConnOpts {
            host: "h".into(),
            user: "u".into(),
            password: None,
            port: None,
            key_file: None,
            host_key_fingerprint: None,
            known_hosts: None,
            accept_any_host_key: false,
            timeout: None,
            json: false,
        };
        tweak(&mut conn);
        conn
    }
}
