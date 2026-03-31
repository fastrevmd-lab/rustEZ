# rustEZ

Rust replacement for Juniper PyEZ. Async-first Junos device automation built on [rustnetconf](https://github.com/fastrevmd-lab/rustnetconf).

Workspace crates: `rustez` (core library), `rustez-cli`, `rustez-py`.

## Build Commands

```sh
cargo check                     # workspace type-check
cargo test -p rustez            # unit tests (no device needed)
cargo clippy -p rustez          # lint
cargo doc -p rustez             # generate docs
```

## Integration Tests

Gated behind `#[ignore]` and env vars. Requires a reachable vSRX:

```sh
RUSTEZ_VSRX_HOST=<DEVICE_IP> \
RUSTEZ_VSRX_USER=<USERNAME> \
RUSTEZ_VSRX_KEY=~/.ssh/<KEY_FILE> \
  cargo test -p rustez -- --ignored
```

Auth: set `RUSTEZ_VSRX_KEY` for key-based or `RUSTEZ_VSRX_PASS` for password auth.

## Code Conventions

- **Async-first** — tokio runtime, all device I/O is async.
- **Doc comments** — all public functions need `///` doc comments (JSDoc-style).
- **Early returns** over nested if/else.
- **Descriptive variable names** — no single-letter vars except loop iterators.
- **quick-xml lifetime gotcha** — always bind `tag.local_name()` to a `let` before calling `.as_ref()`. The temporary must outlive the borrow.
- **Error types** — `RustEzError` is the single error enum. Wraps `NetconfError` via `#[from]`. Use `thiserror` derive.
- **Per-RPC timeouts** — wrap every `client.rpc()` / `client.commit()` call in `tokio::time::timeout`. Default 30s.
- **Config loading** — uses `client.load_configuration(action, format, config)` from rustnetconf 0.6. `build_load_xml()` retained only for `rollback()` and `load_with_warnings()`.
- **ConfigPayload::Set** — maps to `LoadAction::Set, LoadFormat::Text`.
- **Cluster auto-open** — `ConfigManager::load()` auto-calls `open_configuration(Private)` on clustered devices. `unlock()` auto-closes it. State tracked on `Device.config_db_open`.
- **Warnings** — `RpcExecutor::call_with_warnings()` / `call_xml_with_warnings()` return `(String, Vec<RpcErrorInfo>)`. `ConfigManager::load_with_warnings()` does the same for config loads.

## Architecture

- **Device** owns `Option<Client>` from rustnetconf. `None` means closed.
- **RpcExecutor** and **ConfigManager** are transient `&'a mut Client` borrows created per-operation via `dev.rpc()` / `dev.config()`.
- **Facts** gathered via 3 sequential RPCs (`get-software-information`, `get-chassis-inventory`, `get-route-engine-information`), parsed with quick-xml event reader. Includes `is_cluster: bool` (true when multi-RE wrapper detected).
- **Multi-RE** — `unwrap_multi_re()` detects `<multi-routing-engine-results>` wrapper and splits into per-RE content. Also drives cluster detection.
- **Personality** — detected from model string via case-insensitive prefix/substring matching. Order matters (e.g., `vmx` before `mx`).
- **DeviceBuilder** — builder pattern for connection setup. Supports `.no_facts()` to skip auto-gathering.

## Testing

- **Unit tests** use canned XML strings — no device connection needed.
- **Integration tests** use `serial_test::serial` for sequential execution. vSRX limits concurrent NETCONF sessions to 3.
- **Idempotent config tests** — use timestamped hostnames (`rustez-it3-{epoch}`) so there's always a diff to commit.
- Test modules live in each source file (`#[cfg(test)] mod tests`). Integration tests in `rustez/tests/integration_vsrx.rs`.
