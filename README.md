# rustEZ

A Rust replacement for [Juniper PyEZ](https://github.com/Juniper/py-junos-eznc) — async-first Junos device automation built on [rustnetconf](https://github.com/fastrevmd-lab/rustnetconf).

## Why rustEZ?

PyEZ is the de facto Python library for Junos automation. It works, but:

- **Slow at scale** — synchronous, single-threaded. Managing hundreds of devices is painful
- **Runtime errors** — dynamic typing means bugs surface in production, not at compile time
- **No real concurrency** — threading is bolted on, not native

rustEZ gives you the same Junos automation capabilities with:

- **10-100x faster** — async Rust with tokio for parallel operations across thousands of devices
- **Compile-time safety** — typed RPCs, typed facts, typed configs. Wrong RPC? The compiler tells you
- **Native async concurrency** — `tokio::join!` across 1000 devices is one line of code

## Architecture

```
rustez/           Core library — Device, Facts, Config, RPC, operational data
rustez-cli/       CLI binary — Junos automation from the terminal
rustez-py/        Python bindings via PyO3 — pip install rustez
```

Built on [rustnetconf](https://github.com/fastrevmd-lab/rustnetconf) for NETCONF transport, SSH (via russh), connection pooling, vendor profiles, and event notifications (RFC 5277).

## Quick Start (Library)

```rust
use rustez::{Device, ConfigPayload};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect and gather facts
    let mut dev = Device::connect("10.0.0.1")
        .username("admin")
        .password("secret")
        .open()
        .await?;

    let facts = dev.facts().await?;
    println!("{} running Junos {}", facts.hostname, facts.version);

    // Push a config change
    let mut config = dev.config()?;
    config.lock().await?;
    config.load(ConfigPayload::Text(
        "system { host-name new-hostname; }".into()
    )).await?;

    if let Some(diff) = config.diff().await? {
        println!("Changes:\n{diff}");
        config.commit().await?;
    }
    config.unlock().await?;

    // Run an operational RPC
    let output = dev.cli("show interfaces terse").await?;
    println!("{output}");

    dev.close().await?;
    Ok(())
}
```

## Platform Session Limits

Some Junos platforms limit the number of concurrent NETCONF sessions. Exceeding
the limit causes connection resets.

| Platform | Max Concurrent Sessions |
|----------|------------------------|
| vSRX | 3 |
| SRX (branch) | 3 |
| MX / EX / QFX | 8+ (varies by model) |

When automating multiple operations against the same device, keep your
concurrent connections within these limits. The v0.3 `DevicePool` will
auto-detect platform personality and enforce the correct ceiling
automatically.

## Quick Start (CLI)

```bash
# Gather device facts
rustez facts 10.0.0.1 -u admin -p secret

# Run a show command
rustez rpc 10.0.0.1 "show interfaces terse" -u admin

# Push a config
rustez config apply 10.0.0.1 -f config.set -u admin
```

## Quick Start (Python)

```python
from rustez import Device

async def main():
    dev = await Device.connect("10.0.0.1", username="admin", password="secret")
    facts = await dev.facts()
    print(f"{facts.hostname} running Junos {facts.version}")
    await dev.close()
```

## Platform Support (Python)

PyPI wheels are published for **Linux x86_64 only**:

| Platform | Wheel | Status |
|----------|-------|--------|
| Linux x86_64 (glibc) | `manylinux` | Supported |
| Linux x86_64 (musl/Alpine) | `musllinux_1_2` | Supported |
| Linux aarch64 | — | Not supported |
| macOS / Windows | — | Not supported |

For unsupported platforms, build from source with [maturin](https://github.com/PyO3/maturin):

```bash
pip install maturin
git clone https://github.com/fastrevmd-lab/rustEZ.git
cd rustEZ && maturin build --release -m rustez-py/Cargo.toml
pip install target/wheels/*.whl
```

## Tested Platforms

Verified on a real device with all integration tests passing:

| Platform | Junos Version | NETCONF | Tests |
|----------|--------------|---------|-------|
| vSRX | 24.4R1.9 | 1.0 (EOM) | connect, facts, cli, config load/diff/commit/rollback, RFC 5277 event notifications |

## Roadmap

| Phase | Version | Scope |
|-------|---------|-------|
| 1 | v0.1 | Device, Facts, RPC, Config (load/diff/commit/rollback) |
| 2 | v0.2 | Typed operational data (interfaces, routes, ARP, LLDP), CLI |
| 3 | v0.3 | Software management, filesystem, shell, SCP, DevicePool with per-platform session limits |
| 4 | v0.4 | Python bindings via PyO3 |
| 5 | v1.0 | YANG codegen, TUI, config drift detection, 1000+ device scale |

## PyEZ Comparison

| Feature | PyEZ | rustEZ |
|---------|------|--------|
| Language | Python | Rust (with Python bindings) |
| Concurrency | Threading (painful) | Async/await (native) |
| Type safety | Runtime errors | Compile-time checks |
| NETCONF library | ncclient | rustnetconf (async, pure Rust) |
| SSH library | paramiko (OpenSSL) | russh (pure Rust) |
| Config templating | Jinja2 | Tera |
| Operational data | YAML Tables/Views | Typed Rust structs (serde) |
| Multi-vendor | No (Junos only) | No (Junos only) |

## Dependencies

### rustez (core library)

| Crate | Version | Purpose |
|-------|---------|---------|
| [rustnetconf](https://github.com/fastrevmd-lab/rustnetconf) | 0.8.2 | NETCONF client (SSH transport, RFC 6241/5277) |
| [tokio](https://crates.io/crates/tokio) | 1.50 | Async runtime |
| [quick-xml](https://crates.io/crates/quick-xml) | 0.37 | XML parsing |
| [thiserror](https://crates.io/crates/thiserror) | 2.0 | Error derive macros |
| [tracing](https://crates.io/crates/tracing) | 0.1 | Structured logging |
| [serial_test](https://crates.io/crates/serial_test) | 3.4 | Sequential integration tests (dev only) |

### rustez-py (Python bindings)

| Crate | Version | Purpose |
|-------|---------|---------|
| [pyo3](https://crates.io/crates/pyo3) | 0.24 | Python FFI bindings |
| rustez | 0.8.2 | Core library |
| rustnetconf | 0.8.2 | NETCONF client |
| tokio | 1.50 | Async runtime |

Python runtime dependency: [lxml](https://pypi.org/project/lxml/) >= 4.9.0

## Security Audit

Last audited: 2026-04-12 via `cargo audit`

| Severity | Crate | Advisory | Description | Fix Available |
|----------|-------|----------|-------------|---------------|
| Medium (5.9) | `rsa` 0.10.0-rc.17 | [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071) | Marvin Attack — potential key recovery through timing sidechannels | No upstream fix yet |
| Warning | `rand` 0.8.5 | [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097) | Unsound with custom logger using `rand::rng()` | Transitive via `pageant` |

Both are transitive dependencies through `russh` (used by rustnetconf for SSH transport). Neither is directly exploitable in rustEZ's use case — connections are to managed network devices, not public-facing services. These will resolve when upstream `russh` updates its dependency tree.

Run `cargo audit` to check for the latest advisories.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
