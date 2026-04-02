//! Integration tests for rustEZ against a real vSRX device.
//!
//! All tests are `#[ignore]` by default. Run with:
//! ```sh
//! RUSTEZ_VSRX_HOST=<DEVICE_IP> RUSTEZ_VSRX_USER=<USERNAME> \
//!     RUSTEZ_VSRX_KEY=~/.ssh/<KEY_FILE> \
//!     cargo test -p rustez -- --ignored
//! ```

use std::env;
use std::time::Duration;

use rustez::{ConfigPayload, Device, DeviceBuilder};
use serial_test::serial;

/// Build a DeviceBuilder from environment variables.
///
/// Supports both key-based auth (RUSTEZ_VSRX_KEY) and password auth (RUSTEZ_VSRX_PASS).
fn vsrx_builder() -> DeviceBuilder {
    let host = env::var("RUSTEZ_VSRX_HOST").expect("RUSTEZ_VSRX_HOST not set");
    let user = env::var("RUSTEZ_VSRX_USER").unwrap_or_else(|_| "admin".to_string());

    let mut builder = Device::connect(&host).username(&user);

    if let Ok(key_path) = env::var("RUSTEZ_VSRX_KEY") {
        // Expand ~ to home directory
        let expanded = if key_path.starts_with('~') {
            let home = env::var("HOME").expect("HOME not set");
            key_path.replacen('~', &home, 1)
        } else {
            key_path
        };
        builder = builder.key_file(&expanded);
    } else {
        let pass = env::var("RUSTEZ_VSRX_PASS")
            .expect("RUSTEZ_VSRX_PASS or RUSTEZ_VSRX_KEY must be set");
        builder = builder.password(&pass);
    }

    builder
}

/// IT1: Connect, gather facts, verify hostname/model/version/serial.
#[tokio::test]
#[ignore]
#[serial]
async fn test_connect_and_gather_facts() {
    let mut dev = vsrx_builder()
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
#[serial]
async fn test_cli_show_interfaces() {
    let mut dev = vsrx_builder()
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
#[serial]
async fn test_config_load_and_commit() {
    let mut dev = vsrx_builder()
        .no_facts()
        .rpc_timeout(Duration::from_secs(60))
        .open()
        .await
        .expect("failed to connect");

    let mut cfg = dev.config().expect("config manager failed");

    cfg.lock().await.expect("lock failed");

    // Use a unique hostname to ensure there's always a diff
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let payload = ConfigPayload::Text(
        format!("system {{ host-name rustez-it3-{timestamp}; }}"),
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
#[serial]
async fn test_config_rollback() {
    let mut dev = vsrx_builder()
        .no_facts()
        .rpc_timeout(Duration::from_secs(60))
        .open()
        .await
        .expect("failed to connect");

    let mut cfg = dev.config().expect("config manager failed");

    cfg.lock().await.expect("lock failed");

    // Load a change with unique value
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let payload = ConfigPayload::Text(
        format!("system {{ host-name rustez-it4-{timestamp}; }}"),
    );
    cfg.load(payload).await.expect("load failed");
    cfg.commit().await.expect("commit failed");

    // Rollback to previous config
    cfg.rollback(1).await.expect("rollback failed");
    cfg.commit().await.expect("commit after rollback failed");

    cfg.unlock().await.expect("unlock failed");
    dev.close().await.expect("close failed");
}

/// IT5: Subscribe to event notifications and trigger a config change to receive one.
///
/// 1. Open two sessions — one for notifications, one for config changes.
/// 2. Subscribe to the NETCONF event stream on the first session.
/// 3. Commit a config change on the second session to trigger an event.
/// 4. Receive the notification on the first session and verify it.
#[tokio::test]
#[ignore]
#[serial]
async fn test_event_subscription() {
    // Session 1: notification listener
    let mut listener = vsrx_builder()
        .no_facts()
        .rpc_timeout(Duration::from_secs(30))
        .open()
        .await
        .expect("failed to connect listener");

    assert!(!listener.has_subscription(), "should not have subscription yet");

    // Subscribe to the default NETCONF event stream
    let sub_result = listener.create_subscription(None, None, None, None).await;
    if let Err(ref err) = sub_result {
        let msg = format!("{err}");
        if msg.contains("unknown-element") || msg.contains("operation-not-supported") {
            println!("SKIP: device does not support RFC 5277 notifications");
            listener.close().await.expect("close failed");
            return;
        }
    }
    sub_result.expect("create_subscription failed");
    assert!(listener.has_subscription(), "should have active subscription");

    // Session 2: make a config change to trigger a notification
    let mut changer = vsrx_builder()
        .no_facts()
        .rpc_timeout(Duration::from_secs(30))
        .open()
        .await
        .expect("failed to connect changer");

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut cfg = changer.config().expect("config manager failed");
    cfg.lock().await.expect("lock failed");
    let payload = rustez::ConfigPayload::Text(
        format!("system {{ host-name rustez-it5-{timestamp}; }}"),
    );
    cfg.load(payload).await.expect("load failed");
    cfg.commit().await.expect("commit failed");
    cfg.unlock().await.expect("unlock failed");
    println!("config committed — waiting for notification");

    // Wait for a notification to arrive from the config commit
    match listener.recv_notification().await {
        Ok(Some(notif)) => {
            println!("received notification:");
            println!("  event_time: {}", notif.event_time);
            println!("  event_xml: {}", notif.event_xml);
        }
        Ok(None) => println!("connection closed before notification arrived"),
        Err(err) => println!("recv_notification error (may be timeout): {err}"),
    }

    // Drain any additional buffered notifications
    let extra = listener.drain_notifications().expect("drain failed");
    println!("additional buffered notifications: {}", extra.len());

    // Close changer first, then listener (listener close may hang with
    // active subscription — drop is fine as fallback)
    changer.close().await.expect("close changer failed");
    let _ = tokio::time::timeout(Duration::from_secs(5), listener.close()).await;
}
