//! `rustez config ...` handlers.

use rustez::{ConfigPayload, Device};

use crate::cli::{
    ConfigCommitArgs, ConfigConfirmArgs, ConfigFormat, ConfigLoadArgs, ConfigRollbackArgs,
};
use crate::connect::build_device;
use crate::error::{CliError, ErrorKind, Phase};
use crate::output::CommandData;

/// Read a config file into a `ConfigPayload` for the requested format.
fn read_payload(file: &str, format: ConfigFormat) -> Result<ConfigPayload, CliError> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| CliError::new(ErrorKind::Usage, format!("cannot read {file}: {e}")))?;
    Ok(match format {
        ConfigFormat::Set => ConfigPayload::Set(content),
        ConfigFormat::Text => ConfigPayload::Text(content),
        ConfigFormat::Xml => ConfigPayload::Xml(content),
    })
}

/// Lock, load (capturing warnings). On error, the caller closes the device,
/// which releases the candidate lock — so no explicit unlock on the error path.
async fn lock_and_load(dev: &mut Device, payload: ConfigPayload) -> Result<Vec<String>, CliError> {
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    cfg.lock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    let (_resp, warnings) = cfg
        .load_with_warnings(payload)
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    Ok(warnings.iter().map(|w| w.message.clone()).collect())
}

/// `config apply` — load and commit (the simple convenience verb).
pub async fn apply(args: &ConfigLoadArgs) -> Result<CommandData, CliError> {
    let payload = read_payload(&args.file, args.format)?;
    let mut dev = build_device(&args.conn, false).await?;
    let result = apply_inner(&mut dev, payload, None, None).await;
    let _ = dev.close().await;
    result
}

/// `config commit` — load and commit with optional confirm timer/comment.
pub async fn commit(args: &ConfigCommitArgs) -> Result<CommandData, CliError> {
    let payload = read_payload(&args.file, args.format)?;
    let mut dev = build_device(&args.conn, false).await?;
    let result = apply_inner(
        &mut dev,
        payload,
        args.confirm_minutes,
        args.comment.as_deref(),
    )
    .await;
    let _ = dev.close().await;
    result
}

/// Shared load + commit + unlock used by `apply` and `commit`.
async fn apply_inner(
    dev: &mut Device,
    payload: ConfigPayload,
    confirm_minutes: Option<u32>,
    comment: Option<&str>,
) -> Result<CommandData, CliError> {
    let warnings = lock_and_load(dev, payload).await?;
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    let commit_result = if let Some(mins) = confirm_minutes {
        cfg.commit_confirmed(mins * 60).await
    } else if let Some(c) = comment {
        cfg.commit_with_comment(c).await
    } else {
        cfg.commit().await
    };
    commit_result.map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    cfg.unlock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    Ok(CommandData::Commit {
        loaded: true,
        committed: true,
        confirm_minutes,
        warnings,
    })
}

/// `config commit-check` — load and validate without committing.
pub async fn commit_check(args: &ConfigLoadArgs) -> Result<CommandData, CliError> {
    let payload = read_payload(&args.file, args.format)?;
    let mut dev = build_device(&args.conn, false).await?;
    let result = commit_check_inner(&mut dev, payload).await;
    let _ = dev.close().await;
    result
}

async fn commit_check_inner(
    dev: &mut Device,
    payload: ConfigPayload,
) -> Result<CommandData, CliError> {
    let warnings = lock_and_load(dev, payload).await?;
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    cfg.commit_check()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    cfg.unlock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    Ok(CommandData::CommitCheck {
        loaded: true,
        check_passed: true,
        warnings,
    })
}

/// `config diff` — load a file and return the candidate diff (no commit).
pub async fn diff(args: &ConfigLoadArgs) -> Result<CommandData, CliError> {
    let payload = read_payload(&args.file, args.format)?;
    let mut dev = build_device(&args.conn, false).await?;
    let result = diff_inner(&mut dev, payload).await;
    let _ = dev.close().await;
    result
}

async fn diff_inner(dev: &mut Device, payload: ConfigPayload) -> Result<CommandData, CliError> {
    let _warnings = lock_and_load(dev, payload).await?;
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    let diff = cfg
        .diff()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    cfg.unlock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Load))?;
    Ok(CommandData::Diff { diff })
}

/// `config confirm` — bare confirming commit for a prior confirmed commit.
pub async fn confirm(args: &ConfigConfirmArgs) -> Result<CommandData, CliError> {
    let mut dev = build_device(&args.conn, false).await?;
    let result = confirm_inner(&mut dev).await;
    let _ = dev.close().await;
    result
}

async fn confirm_inner(dev: &mut Device) -> Result<CommandData, CliError> {
    // A bare `<commit-configuration/>` against an empty candidate confirms a
    // pending `commit confirmed` from a prior session, cancelling its
    // auto-rollback timer. We intentionally do not lock/load here.
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    cfg.commit()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Commit))?;
    Ok(CommandData::Confirm { committed: true })
}

/// `config rollback` — roll back to an id and commit.
pub async fn rollback(args: &ConfigRollbackArgs) -> Result<CommandData, CliError> {
    let mut dev = build_device(&args.conn, false).await?;
    let result = rollback_inner(&mut dev, args.id).await;
    let _ = dev.close().await;
    result
}

async fn rollback_inner(dev: &mut Device, id: u32) -> Result<CommandData, CliError> {
    let mut cfg = dev
        .config()
        .map_err(|e| CliError::from_rustez(&e, Phase::Rollback))?;
    cfg.lock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Rollback))?;
    cfg.rollback(id)
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Rollback))?;
    cfg.commit()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Rollback))?;
    cfg.unlock()
        .await
        .map_err(|e| CliError::from_rustez(&e, Phase::Rollback))?;
    Ok(CommandData::Rollback {
        rolled_back: true,
        id,
    })
}
