//! Command-line grammar for rustez-cli (clap derive).

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Top-level CLI entrypoint.
#[derive(Parser, Debug)]
#[command(
    name = "rustez",
    version,
    about = "Junos device automation from the terminal"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Gather and print device facts.
    Facts(FactsArgs),
    /// Run an operational CLI command on the device.
    Rpc(RpcArgs),
    /// Configuration operations.
    Config(ConfigArgs),
}

/// Connection options shared by every command.
#[derive(Args, Debug)]
pub struct ConnOpts {
    /// Device hostname or IP.
    pub host: String,
    /// Login username.
    #[arg(short, long)]
    pub user: String,
    /// Login password (insecure: visible in process list — prefer $RUSTEZ_PASSWORD).
    #[arg(short, long)]
    pub password: Option<String>,
    /// NETCONF port (library default if unset).
    #[arg(long)]
    pub port: Option<u16>,
    /// Path to an SSH private key for key-based auth.
    #[arg(long)]
    pub key_file: Option<String>,
    /// Expected SSH host-key fingerprint (e.g. SHA256:...).
    #[arg(long, group = "hostkey")]
    pub host_key_fingerprint: Option<String>,
    /// Path to a known_hosts file for host-key verification.
    #[arg(long, group = "hostkey")]
    pub known_hosts: Option<String>,
    /// Accept any host key (LAB ONLY — disables verification).
    #[arg(long, group = "hostkey")]
    pub accept_any_host_key: bool,
    /// Per-RPC timeout in seconds.
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Emit machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Output format for `rpc`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum RpcFormat {
    Text,
    Xml,
}

impl RpcFormat {
    /// The string Junos expects in the `<command format="...">` attribute.
    pub fn as_junos(self) -> &'static str {
        match self {
            RpcFormat::Text => "text",
            RpcFormat::Xml => "xml",
        }
    }
}

/// Config load payload format.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ConfigFormat {
    Set,
    Text,
    Xml,
}

#[derive(Args, Debug)]
pub struct FactsArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
}

#[derive(Args, Debug)]
pub struct RpcArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
    /// Operational CLI command, e.g. "show interfaces terse".
    pub rpc_command: String,
    /// Output format requested from the device.
    #[arg(long, value_enum, default_value_t = RpcFormat::Text)]
    pub format: RpcFormat,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Load a config file and commit it.
    Apply(ConfigLoadArgs),
    /// Load a config file and show the candidate diff (no commit).
    Diff(ConfigLoadArgs),
    /// Load a config file and validate it (no commit).
    CommitCheck(ConfigLoadArgs),
    /// Load a config file and commit, with optional confirm timer/comment.
    Commit(ConfigCommitArgs),
    /// Issue a bare confirming commit (confirms a prior `commit --confirm-minutes`).
    Confirm(ConfigConfirmArgs),
    /// Roll back to a previous configuration and commit.
    Rollback(ConfigRollbackArgs),
}

/// Args for commands that load a file: apply, diff, commit-check.
#[derive(Args, Debug)]
pub struct ConfigLoadArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
    /// Path to the configuration file to load.
    #[arg(short, long)]
    pub file: String,
    /// Format of the configuration file.
    #[arg(long, value_enum, default_value_t = ConfigFormat::Set)]
    pub format: ConfigFormat,
}

#[derive(Args, Debug)]
pub struct ConfigCommitArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
    /// Path to the configuration file to load.
    #[arg(short, long)]
    pub file: String,
    /// Format of the configuration file.
    #[arg(long, value_enum, default_value_t = ConfigFormat::Set)]
    pub format: ConfigFormat,
    /// Arm a confirmed commit that auto-rolls-back after N minutes unless confirmed.
    #[arg(long)]
    pub confirm_minutes: Option<u32>,
    /// Commit log comment. Not supported together with --confirm-minutes.
    #[arg(long, conflicts_with = "confirm_minutes")]
    pub comment: Option<String>,
}

#[derive(Args, Debug)]
pub struct ConfigConfirmArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
}

#[derive(Args, Debug)]
pub struct ConfigRollbackArgs {
    #[command(flatten)]
    pub conn: ConnOpts,
    /// Rollback id (0 = previous commit).
    #[arg(long, default_value_t = 0)]
    pub id: u32,
}

impl Cli {
    /// The shared connection options for whichever command was selected.
    pub fn conn(&self) -> &ConnOpts {
        match &self.command {
            Command::Facts(a) => &a.conn,
            Command::Rpc(a) => &a.conn,
            Command::Config(c) => match &c.command {
                ConfigCommand::Apply(a)
                | ConfigCommand::Diff(a)
                | ConfigCommand::CommitCheck(a) => &a.conn,
                ConfigCommand::Commit(a) => &a.conn,
                ConfigCommand::Confirm(a) => &a.conn,
                ConfigCommand::Rollback(a) => &a.conn,
            },
        }
    }

    /// Stable command name used in the JSON envelope `command` field.
    pub fn command_name(&self) -> &'static str {
        match &self.command {
            Command::Facts(_) => "facts",
            Command::Rpc(_) => "rpc",
            Command::Config(c) => match &c.command {
                ConfigCommand::Apply(_) => "config apply",
                ConfigCommand::Diff(_) => "config diff",
                ConfigCommand::CommitCheck(_) => "config commit-check",
                ConfigCommand::Commit(_) => "config commit",
                ConfigCommand::Confirm(_) => "config confirm",
                ConfigCommand::Rollback(_) => "config rollback",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_facts_with_required_args() {
        let cli = Cli::try_parse_from(["rustez", "facts", "10.0.0.1", "-u", "admin"]).unwrap();
        assert_eq!(cli.command_name(), "facts");
        assert_eq!(cli.conn().host, "10.0.0.1");
        assert_eq!(cli.conn().user, "admin");
        assert!(!cli.conn().json);
    }

    #[test]
    fn facts_requires_user() {
        let res = Cli::try_parse_from(["rustez", "facts", "10.0.0.1"]);
        assert!(res.is_err());
    }

    #[test]
    fn parses_rpc_command_and_format() {
        let cli = Cli::try_parse_from([
            "rustez",
            "rpc",
            "10.0.0.1",
            "show interfaces terse",
            "-u",
            "admin",
            "--format",
            "xml",
        ])
        .unwrap();
        match &cli.command {
            Command::Rpc(a) => {
                assert_eq!(a.rpc_command, "show interfaces terse");
                assert_eq!(a.format.as_junos(), "xml");
            }
            _ => panic!("expected rpc"),
        }
    }

    #[test]
    fn host_key_flags_are_mutually_exclusive() {
        let res = Cli::try_parse_from([
            "rustez",
            "facts",
            "10.0.0.1",
            "-u",
            "admin",
            "--accept-any-host-key",
            "--known-hosts",
            "/tmp/kh",
        ]);
        assert!(res.is_err(), "two host-key flags should conflict");
    }

    #[test]
    fn parses_config_commit_with_confirm() {
        let cli = Cli::try_parse_from([
            "rustez",
            "config",
            "commit",
            "10.0.0.1",
            "-u",
            "admin",
            "-f",
            "c.set",
            "--confirm-minutes",
            "5",
            "--json",
        ])
        .unwrap();
        assert_eq!(cli.command_name(), "config commit");
        assert!(cli.conn().json);
        match &cli.command {
            Command::Config(c) => match &c.command {
                ConfigCommand::Commit(a) => assert_eq!(a.confirm_minutes, Some(5)),
                _ => panic!("expected commit"),
            },
            _ => panic!("expected config"),
        }
    }

    #[test]
    fn confirm_minutes_and_comment_are_mutually_exclusive() {
        let res = Cli::try_parse_from([
            "rustez",
            "config",
            "commit",
            "10.0.0.1",
            "-u",
            "admin",
            "-f",
            "c.set",
            "--confirm-minutes",
            "5",
            "--comment",
            "change-123",
        ]);
        assert!(
            res.is_err(),
            "--comment with --confirm-minutes should conflict (comment would be silently dropped)"
        );
    }

    #[test]
    fn rollback_id_defaults_to_zero() {
        let cli = Cli::try_parse_from(["rustez", "config", "rollback", "10.0.0.1", "-u", "admin"])
            .unwrap();
        match &cli.command {
            Command::Config(c) => match &c.command {
                ConfigCommand::Rollback(a) => assert_eq!(a.id, 0),
                _ => panic!("expected rollback"),
            },
            _ => panic!("expected config"),
        }
    }
}
