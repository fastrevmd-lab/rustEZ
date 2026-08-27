//! `rustez rpc` handler.

use crate::cli::RpcArgs;
use crate::connect::build_device;
use crate::error::{CliError, Phase};
use crate::output::CommandData;

/// Connect (no facts) and run an operational CLI command.
pub async fn run(args: &RpcArgs) -> Result<CommandData, CliError> {
    let mut dev = build_device(&args.conn, false).await?;
    let format = args.format.as_junos();
    let output = {
        let mut executor = dev
            .rpc()
            .map_err(|e| CliError::from_rustez(&e, Phase::Rpc))?;
        executor
            .cli(&args.rpc_command, format)
            .await
            .map_err(|e| CliError::from_rustez(&e, Phase::Rpc))?
    };
    let _ = dev.close().await;
    Ok(CommandData::Rpc {
        output,
        format: format.to_string(),
    })
}
