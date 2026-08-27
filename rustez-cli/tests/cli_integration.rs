//! Integration test for rustez-cli against a real vSRX.
//!
//! Ignored by default. Run with:
//! ```sh
//! RUSTEZ_VSRX_HOST=<IP> RUSTEZ_VSRX_USER=<USER> RUSTEZ_VSRX_PASS=<PASS> \
//!     cargo test -p rustez-cli -- --ignored
//! ```

use std::env;
use std::process::Command;

/// IT: `rustez facts --json` returns exit 0 and a parseable success envelope.
#[test]
#[ignore]
fn facts_json_against_vsrx() {
    let host = env::var("RUSTEZ_VSRX_HOST").expect("RUSTEZ_VSRX_HOST not set");
    let user = env::var("RUSTEZ_VSRX_USER").unwrap_or_else(|_| "admin".to_string());
    let pass = env::var("RUSTEZ_VSRX_PASS").expect("RUSTEZ_VSRX_PASS not set");

    let output = Command::new(env!("CARGO_BIN_EXE_rustez-cli"))
        .args([
            "facts",
            &host,
            "-u",
            &user,
            "-p",
            &pass,
            "--accept-any-host-key",
            "--json",
        ])
        .output()
        .expect("failed to run rustez-cli");

    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout not utf8");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout not valid JSON");
    assert_eq!(v["ok"], true);
    assert_eq!(v["command"], "facts");
    assert!(
        v["data"]["hostname"]
            .as_str()
            .is_some_and(|h| !h.is_empty()),
        "expected non-empty hostname, got {v}"
    );
}
