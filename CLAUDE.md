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
- **Config loading** — uses `client.load_configuration(action, format, config)` from rustnetconf 0.6. `build_load_xml()` is retained for `load_with_warnings()` only — that is the one candidate-modifying call still on the raw `rpc_with_warnings()` escape hatch, because rustnetconf has no `rpc_candidate_change_with_warnings`. `rollback()` uses the typed `client.rollback_configuration()`.
- **ConfigPayload::Set** — maps to `LoadAction::Set, LoadFormat::Text`.
- **Namespace prefix** — `build_load_xml()` uses `nc:` prefix on XML elements to match rustnetconf 0.7.1's `<nc:rpc>` envelope. Required for Junos 24.4 compatibility.
- **Cluster auto-open** — `ConfigManager::load()` auto-calls `open_configuration(Private)` on clustered devices. `unlock()` auto-closes it. State tracked on `Device.config_db_open`.
- **Candidate-dirty tracking** — rustnetconf only sends `<discard-changes/>` on close when *this* session dirtied the shared
  candidate. Typed ops (`load_configuration`, `rollback_configuration`, `edit_config`) mark it automatically; the raw `rpc()` /
  `rpc_with_warnings()` path does not. **Never hand-mark.** Use an atomic candidate-change call, which validates → preflights →
  marks → sends as one step: `client.rpc_candidate_change()` (via `RpcExecutor::call_xml_candidate_change()`, Python
  `Device.rpc.raw_xml_candidate_change()`), or `client.rpc_candidate_change_with_warnings()` when warnings are needed
  (`ConfigManager::load_with_warnings()`). Surfaced to callers as `Device::touched_candidate()`.
- **A false dirty mark is worse than a missing one** — a missing mark leaves a dirty candidate that blocks the next lock; a false
  mark makes `close()` discard *another operator's* uncommitted work, which is the bug this tracking exists to prevent. The raw
  `rpc*` calls reject a malformed fragment locally *without sending*, and `ConfigPayload::Xml` passes caller XML through
  unescaped, so hand-marking before one of them is a live way to reintroduce the bug. `mark_candidate_dirty()` exists but rustEZ
  no longer calls it anywhere — the typed calls own both **setting and clearing** the mark, and nothing should touch it outside
  them. A commit on the raw `rpc()` path cannot clear the flag, which is why `commit_with_comment()` uses the typed
  `client.commit_configuration_with_log()`.
- **Never escape what rustnetconf already escapes** — `commit_configuration_with_log()` XML-escapes the log text, and
  `load_configuration()` escapes its Text/Set payload. Escaping again in rustEZ double-escapes it and the value reaches the
  device mangled. rustEZ only escapes what *it* embeds itself, which is now just `build_load_xml()`'s Text and Set payloads.
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

## Release Process

Two registries, two paths. PyPI is automated on tag push; crates.io is manual by design (low release cadence, avoids the supply-chain surface of a CI token).

1. **Bump versions** in all three manifests — must agree or `scripts/check_versions.py` fails CI:
   - `rustez/Cargo.toml`
   - `rustez-py/Cargo.toml`
   - `rustez-py/pyproject.toml`
2. **Update `rustez/CHANGELOG.md`** — add `## [X.Y.Z] — YYYY-MM-DD` section, document breaking changes prominently, add the compare link at the bottom.
3. **Commit + push to `main`** — wait for CI to go green.
4. **Tag the release** — annotated tag with summary of changes:
   ```sh
   git tag -a vX.Y.Z -m "rustez X.Y.Z\n\n<summary>"
   git push origin vX.Y.Z
   ```
   This triggers `.github/workflows/publish-pypi.yml` automatically. The `pypi` GitHub environment provides the OIDC trusted publisher — no API token needed.
5. **Publish to crates.io** — from a clean working tree on the tagged commit:
   ```sh
   cargo publish -p rustez --dry-run   # validate packaging
   cargo publish -p rustez             # actual upload — irreversible (yank-only)
   ```
   Uses the local token in `~/.cargo/credentials.toml`. `rustez-py` is PyPI-only. `rustez-cli` (0.1.0) is not on crates.io.
6. **Create the GitHub Release** — a tag is not a release, and this step is easy
   to skip because nothing fails without it:
   ```sh
   gh release create vX.Y.Z --title "vX.Y.Z — <short theme>" --notes-file <(...)
   ```
   Draw the notes from the `rustez/CHANGELOG.md` section for that version, and
   title it the way rustnetconf does — `v0.15.0 — smaller error types`, not the
   bare version. Mark it `--latest` only if it really is the newest.

   **Why this is called out:** it was missing from this list for the crate's
   whole history, and the result was 13 tags and zero releases while the
   changelog, tags, and both registries stayed in sync. Nothing breaks — the
   crate just has no readable release history, and anyone watching the repo for
   releases gets nothing.
7. **Verify all three surfaces:**
   ```sh
   curl -sH "User-Agent: rustez-release-check" https://crates.io/api/v1/crates/rustez | jq '.crate.max_version'
   pip index versions rustez   # or: pip install rustez==X.Y.Z in a fresh venv
   gh release view vX.Y.Z --json tagName,name --jq '.tagName + "  " + .name'
   ```
   Manifest, tag, GitHub Release, and crates.io should all report the same
   version. Drift in any one of them is the thing this checklist exists to
   prevent.

**Yanking:** Use `cargo yank --version X.Y.Z -p rustez` only for broken or unsafe releases. PyPI has no yank for releases — request deletion via the PyPI UI (or release a patch instead).
