//! Integration tests for rustEZ against a real vSRX device.
//!
//! All tests are `#[ignore]` by default. Run with:
//! ```sh
//! RUSTEZ_VSRX_HOST=10.x.x.x RUSTEZ_VSRX_USER=admin RUSTEZ_VSRX_PASS=secret \
//!     cargo test -p rustez -- --ignored
//! ```

use std::env;
use std::time::Duration;

use rustez::{ConfigPayload, Device};

/// Get connection params from environment, or skip the test.
fn vsrx_params() -> (String, String, String) {
    let host = env::var("RUSTEZ_VSRX_HOST").expect("RUSTEZ_VSRX_HOST not set");
    let user = env::var("RUSTEZ_VSRX_USER").unwrap_or_else(|_| "admin".to_string());
    let pass = env::var("RUSTEZ_VSRX_PASS").unwrap_or_else(|_| "admin123".to_string());
    (host, user, pass)
}

/// IT1: Connect, gather facts, verify hostname/model/version/serial.
#[tokio::test]
#[ignore]
async fn test_connect_and_gather_facts() {
    let (host, user, pass) = vsrx_params();

    let mut dev = Device::connect(&host)
        .username(&user)
        .password(&pass)
        .rpc_timeout(Duration::from_secs(60))
        .open()
        .await
        .expect("failed to connect");

    let facts = dev.facts().await.expect("failed to gather facts");

    assert!(!facts.hostname.is_empty(), "hostname should not be empty");
    assert!(!facts.model.is_empty(), "model should not be empty");
    assert!(!facts.version.is_empty(), "version should not be empty");
    assert!(!facts.serial_number.is_empty(), "serial should not be empty");

    println!("hostname: {}", facts.hostname);
    println!("model: {}", facts.model);
    println!("version: {}", facts.version);
    println!("serial: {}", facts.serial_number);
    println!("personality: {}", facts.personality);
    println!("route_engines: {}", facts.route_engines.len());

    dev.close().await.expect("close failed");
}

/// IT2: Run `show interfaces terse` via cli(), verify non-empty output.
#[tokio::test]
#[ignore]
async fn test_cli_show_interfaces() {
    let (host, user, pass) = vsrx_params();

    let mut dev = Device::connect(&host)
        .username(&user)
        .password(&pass)
        .no_facts()
        .open()
        .await
        .expect("failed to connect");

    let output = dev
        .cli("show interfaces terse")
        .await
        .expect("cli failed");

    assert!(!output.is_empty(), "CLI output should not be empty");
    println!("show interfaces terse:\n{output}");

    dev.close().await.expect("close failed");
}

/// IT3: Lock → load set config → diff → commit → unlock → verify change.
#[tokio::test]
#[ignore]
async fn test_config_load_and_commit() {
    let (host, user, pass) = vsrx_params();

    let mut dev = Device::connect(&host)
        .username(&user)
        .password(&pass)
        .no_facts()
        .rpc_timeout(Duration::from_secs(60))
        .open()
        .await
        .expect("failed to connect");

    let mut cfg = dev.config().expect("config manager failed");

    cfg.lock().await.expect("lock failed");

    let payload = ConfigPayload::Set(
        "set system description \"rustEZ integration test\"".to_string(),
    );
    cfg.load(payload).await.expect("load failed");

    let diff = cfg.diff().await.expect("diff failed");
    assert!(diff.is_some(), "diff should show changes");
    println!("diff:\n{}", diff.unwrap());

    cfg.commit().await.expect("commit failed");
    cfg.unlock().await.expect("unlock failed");

    dev.close().await.expect("close failed");
}

/// IT4: Rollback after config change.
#[tokio::test]
#[ignore]
async fn test_config_rollback() {
    let (host, user, pass) = vsrx_params();

    let mut dev = Device::connect(&host)
        .username(&user)
        .password(&pass)
        .no_facts()
        .rpc_timeout(Duration::from_secs(60))
        .open()
        .await
        .expect("failed to connect");

    let mut cfg = dev.config().expect("config manager failed");

    cfg.lock().await.expect("lock failed");

    // Load a change
    let payload = ConfigPayload::Set(
        "set system description \"rollback test\"".to_string(),
    );
    cfg.load(payload).await.expect("load failed");
    cfg.commit().await.expect("commit failed");

    // Rollback to previous config
    cfg.rollback(1).await.expect("rollback failed");
    cfg.commit().await.expect("commit after rollback failed");

    cfg.unlock().await.expect("unlock failed");
    dev.close().await.expect("close failed");
}
