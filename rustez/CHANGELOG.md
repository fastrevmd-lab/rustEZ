# Changelog

All notable changes to the `rustez` crate are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.15.0] — 2026-08-26

### Changed

- **BREAKING: `rustnetconf` raised to `0.15`, which reshapes one error variant.**
  `RpcError::ServerError` became a tuple variant carrying a boxed struct
  ([rustnetconf#66](https://github.com/fastrevmd-lab/rustnetconf/issues/66)). Its
  seven RFC 6241 §4.3 fields moved into a new public `RpcServerError`; the fields
  themselves are unchanged, still public, and still all present.

  rustEZ has no `ServerError` construction or match of its own, but it re-exposes
  the type: `RustEzError::Netconf` wraps `NetconfError` with `#[from]`, so a
  caller matching through it must update.

  ```rust
  // before
  Err(RustEzError::Netconf(NetconfError::Rpc(RpcError::ServerError { tag, message, .. }))) => ...

  // after
  Err(RustEzError::Netconf(NetconfError::Rpc(RpcError::ServerError(e)))) => ... // e.tag, e.message
  ```

  `Display` output is byte-for-byte what it was, so anything matching on the error
  *string* is unaffected.

- **Removed six `#[allow(clippy::result_large_err)]` suppressions**
  (`rustez/src/device.rs` ×4, `rustez/src/rpc.rs` ×2). They existed only because
  `NetconfError` measured exactly 128 bytes — clippy's threshold — which
  `RustEzError` inherited through its `#[from]` variant. Boxing upstream takes
  `NetconfError` to 72, so the lint is satisfied on the merits rather than
  silenced, and every `Result<T, RustEzError>` in the crate stops moving 128
  bytes on the success path.

  This also **unblocks CI.** The suppressions covered 6 sites; clippy 1.98.0
  flagged 34, so `cargo clippy -p rustez -- -D warnings` and the same gate on
  `rustez-py` were both failing on `main` — unnoticed because CI had not run
  since 2026-08-20, before stable moved to 1.98.0.

### Fixed

Two device-facing bugs, both fixed upstream and reaching rustEZ only now because
the floor was still `0.14.3`:

- **A benign Junos warning no longer sinks an entire config load**
  ([rustnetconf#67](https://github.com/fastrevmd-lab/rustnetconf/issues/67), 0.14.5).
  Deleting a statement that is not present returns an `<rpc-error>` carrying only
  severity and message; RFC 6241 makes `error-type` and `error-tag` mandatory and
  Junos omits both, so the reply was rejected outright and the load failed.
  `ConfigManager::load_with_warnings()` is exactly this path, which makes rustEZ
  unusually exposed — it exists to surface warnings, and a warning was fatal.

- **A standalone SRX's commit-check verdict is no longer discarded**
  ([rustnetconf#65](https://github.com/fastrevmd-lab/rustnetconf/issues/65), 0.14.4).
  A single-RE SRX345 answers a commit-check with a closed `<commit-results>`
  followed by a sibling `<ok/>`; the reply failed to parse and a passing check
  was thrown away. The chassis-cluster form already worked, so a device that
  closed the element *correctly* fared worse than one that did not.

### Documentation

- Corrected the dependency tables, which had drifted badly — `rustnetconf` was
  listed as `0.10` against an actual floor of `0.14.3`, alongside stale `pyo3`,
  `quick-xml`, and `rustez` rows, and a missing `serde` entry.
- Rewrote the security-audit section. It advertised RUSTSEC-2023-0071 against
  `rsa` 0.10.0-rc.16 and claimed CI ignored it via `cargo audit --ignore`;
  `rsa` is no longer in the tree, the audit is clean across 237 dependencies,
  and no such ignore exists in the workflow.

## [0.14.3] — 2026-08-20

### Changed

- **Raised the `rustnetconf` floor to `0.14.3`.** No rustEZ source changes —
  this release exists only to stop a fresh resolve from picking a version with
  a bug rustEZ is unusually exposed to.

  rustnetconf 0.14.3 fixes [#61](https://github.com/fastrevmd-lab/rustnetconf/issues/61):
  it tracked "a commit is in flight" in a session field, set before the send and
  cleared after it, so a commit future dropped at its `.await` left the flag set
  for the life of the session. The next *unrelated* transport EOF was then
  reported as `CommitUnknown` — the flag's purpose inverted, since it exists to
  stop a genuinely indeterminate commit being mistaken for a clean I/O failure.

  **Why rustEZ specifically:** every `client.rpc()` / `client.commit()` call here
  is wrapped in `tokio::time::timeout` by convention (see CLAUDE.md), which is
  exactly the cancellation shape that triggered it. A commit that hit its
  per-RPC timeout would poison the next failure on that session.

  The previous `"0.14.2"` requirement is a caret, so existing consumers already
  picked 0.14.3 up on their next `cargo update`. The floor matters for a *fresh*
  resolve against a new lockfile, which could otherwise still select 0.14.2.

## [0.14.2] — 2026-08-20

### Fixed

- **`commit_with_comment()` no longer leaves a stale candidate-dirty mark**
  (#41). It sent `<commit-configuration><log>` through the raw `rpc()` path,
  which cannot *clear* the flag — every clear in rustnetconf is a private
  side effect of a typed call, and there was no typed commit-with-comment. So a
  session stayed marked dirty across a commit that genuinely cleaned the
  candidate, and `close()` discarded afterwards.

  Harmless against that session's own work. Not harmless on the **shared**
  Junos candidate: anything another operator staged between our commit and our
  close was destroyed. This was the last of the three routes back to the bug
  #36 fixed.

  rustnetconf 0.14.2 adds `commit_configuration_with_log()`, which clears the
  flag on success exactly as `commit_configuration()` does.
  `commit_with_comment()` now calls it.

  **Behavioural note for consumers pooling sessions:** a session that commits
  with a comment is now clean, so `touched_candidate()` reports `false` and the
  session is safe to return to the pool. Callers that worked around this by
  closing such sessions while still holding the lock can stop doing so.

### Changed

- Bumped `rustnetconf` to `0.14.2`.
- **`commit_with_comment()` no longer escapes the comment itself.** rustnetconf
  escapes the log text, so rustEZ escaping first would double-escape it and the
  comment would reach the device mangled. `build_commit_with_comment_xml()` and
  its three unit tests are removed; the injection coverage they provided now
  lives upstream, against the code that actually builds the fragment.

### Notes

- No public rustEZ API changed — `commit_with_comment()` keeps its signature.
- `mark_candidate_dirty()` is still called nowhere in rustEZ. As of this
  release the typed calls own both setting *and* clearing the mark.

## [0.14.1] — 2026-08-20

### Fixed

- **`load_with_warnings()` no longer hand-marks the candidate** (#40). 0.14.0
  had to validate the fragment itself and then call `mark_candidate_dirty()`
  before `rpc_with_warnings()`, because rustnetconf had no atomic
  candidate-change call that returns warnings. That workaround closed the
  malformed-XML hole but could not reach rustnetconf's private
  `keepalive_check()` / `ensure_established()` preflight, so a dead or
  unreachable session could still leave a **false** dirty mark — and a false
  mark is the dangerous direction: `close()` acting on it discards another
  operator's uncommitted work on the shared Junos candidate, which is precisely
  the bug 0.14.0 set out to fix.

  rustnetconf 0.14.1 adds `rpc_candidate_change_with_warnings()`, which
  validates, preflights, marks, and only then sends — as one step. The
  hand-rolled sequence is gone, and `mark_candidate_dirty()` is now called
  nowhere in rustEZ: the atomic calls own the mark, so nothing can mark for an
  RPC that never reached the device.

### Changed

- Bumped `rustnetconf` to `0.14.1`. Additive patch upgrade — no behaviour
  changes and no API removals.

### Notes

- No public rustEZ API changed. `commit_with_comment()`'s inverse limitation is
  unchanged and still documented under 0.14.0 — it cannot *clear* the flag on
  the raw `rpc()` path, because there is still no typed commit-with-comment
  upstream. **Resolved in 0.14.2.**

## [0.14.0] — 2026-08-19

### Fixed

- **Closing a session no longer discards another operator's uncommitted work**
  (#36). Junos exposes a **shared** candidate datastore, and `Device::close()`
  previously caused an unconditional `<discard-changes/>`. A session that merely
  read, or whose `<lock>` was refused with `configuration database modified`,
  destroyed exactly the work that caused the refusal — silently, with no error
  and no log. The upstream fix (rustnetconf #55 / 0.14.0) tracks whether *this*
  session dirtied the candidate and discards only then, so the safety property
  is kept without the collateral damage.

- **`load_with_warnings()` now marks the candidate dirty.** It is the one
  candidate-modifying call still on the raw `rpc_with_warnings()` escape hatch,
  because rustnetconf has no `rpc_candidate_change_with_warnings` variant. Under
  0.14.0's conditional discard it would otherwise have left a dirty candidate
  behind at close, blocking the next session's lock. The mark is issued
  **before** the send, so a timeout or partially-applied load still counts —
  but **after** the payload is validated locally, because `rpc_with_warnings()`
  rejects a malformed fragment without sending anything. Marking first would
  leave a false mark on a load that never happened, and `close()` acting on it
  would discard a third party's work — the very bug above, reintroduced.
  `ConfigPayload::Xml` passes caller XML through unescaped, so that path was
  reachable. Validation now runs before the configuration database is opened,
  so an invalid payload fails with no side effects at all.

- **Candidate-modifying RPCs sent through the raw escape hatch are tracked
  again.** `RpcExecutor::call_xml()` and `call_xml_with_warnings()` do not mark
  the candidate, and before 0.14.0 the unconditional discard covered them
  anyway. It no longer does, which silently regressed a documented path — the
  Python `Device.rpc.raw_xml()` docstring gave `<load-configuration
  action="replace">` as its worked example. See *Added* for the replacement.

### Added

- **`RpcExecutor::call_xml_candidate_change()`**, exposed to Python as
  **`Device.rpc.raw_xml_candidate_change()`** — the supported way to send a
  candidate-modifying RPC over the raw path. It routes through rustnetconf's
  `rpc_candidate_change()`, which validates, runs the send preflight, marks, and
  only then writes, so a locally rejected fragment cannot leave a false mark.
  `call_xml()` / `call_xml_with_warnings()` / `raw_xml()` keep their existing
  behaviour and now say plainly in their docs that they do **not** track the
  candidate. This is a deliberate second method rather than a flag on the
  existing one, mirroring upstream, because a flag is too easy to forget.

- **`Device::touched_candidate() -> bool`** (#36) — whether this session has a
  candidate-modifying operation outstanding against the shared datastore. Lets a
  connection-pooling layer decide whether a session is safe to return to the
  pool or must be closed so the discard happens. A closed device returns
  `false`. Exposed to Python as `Device.touched_candidate()` — on the public
  `rustez.Device` wrapper, not only the native extension type.

### Changed

- Bumped `rustnetconf` to `0.14.0`, which also picks up two fixes from the
  intervening patch releases:
  - **0.13.2** — Junos chassis clusters return commit-check/`validate` replies
    whose `<routing-engine>` elements are opened but never closed, which broke
    the strict XML parser. This affected `ConfigManager::commit_check()` on
    clusters directly.
  - **0.13.3** — hardened RPC reply parsing for malformed and partial replies.
- No `Cargo.lock` is committed for this library, so the SSH transport resolves
  to the newest compatible `russh` 0.62.x at build time — which now includes
  the `Channel::data()` backpressure fix that rustnetconf 0.13.3 called out.

### Known limitations

- **`commit_with_comment()` leaves a stale candidate-dirty mark.** It sends
  `<commit-configuration><log>` through the raw `rpc()` path, and a successful
  commit normally *clears* the flag — but rustnetconf exposes neither a typed
  commit-with-comment nor a public flag-clearing method, so the clear cannot
  happen. The session therefore still discards at close. This is **not a
  regression** — 0.13.1 discarded unconditionally regardless — it is an
  improvement this one method does not yet get. The residual risk is narrow: a
  third party would have to dirty the shared candidate in the window between
  our commit and our close.

  **Resolved in 0.14.2**, once rustnetconf grew a typed commit-with-log. This
  entry is left as written because it describes what 0.14.0 actually shipped.

## [0.13.1] — 2026-07-22

### Changed

- Bumped `rustnetconf` dependency to `0.13.1`, which moves the SSH transport
  from **russh 0.61 → 0.62** and off the prerelease (`-rc`) RustCrypto stack
  (#34). No `rustez` source changes were required. The `-rc` crate surface in
  the lockfile shrinks from ~13 crates to only `ssh-key` and its transitive
  `argon2` / `blake2` — all gated on RustCrypto + russh upstream, not clearable
  here.

## [0.13.0] — 2026-07-19

### Fixed

- **`memory_total` is no longer null on vSRX route engines** (#30). vSRX emits
  the total as `<memory-system-total>`, which the parser did not recognize, so
  the value fell through silently. The element is now parsed. Note it arrives
  as a **bare number** (`16323`) where MX/RE-VMX emits a unit-bearing string
  (`<memory-dram-size>4096 MB</memory-dram-size>`); see *Changed* below for how
  this is reconciled.
- **`master_re` is no longer null on standalone devices** (#30). Platforms such
  as vSRX omit `<mastership-state>` entirely, which left `find_master_re`
  returning `None` on every single-RE chassis. A lone RE reporting no mastership
  state is now treated as the master. A lone RE that *explicitly* reports a
  non-master state is left alone — the device's own answer wins over the
  inference — and a multi-RE chassis reporting no state anywhere still yields
  `None` rather than a guess.

  The `RouteEngine.mastership_state` field itself is **not** synthesized: it
  stays `None` on these platforms, reflecting what the device actually said.
  Only the derived `master_re` changes.

### Changed

- **`RouteEngine.memory_total` is normalized to a `"N MB"` string.** Values
  parsed from `<memory-system-total>` gain an explicit ` MB` suffix so the
  field has one shape across platforms rather than exposing per-platform
  formatting to callers. Values that already carry a unit (the existing
  `memory-dram-size` / `memory-installed-size` path) pass through untouched and
  are never double-suffixed. **Callers that string-match `memory_total` exactly
  may need updating**; callers that parse a leading integer are unaffected.
- Bumped `rustnetconf` dependency to `0.13`. No source changes were required —
  the release's `DeviceConfig.vendor` `Box` → `Arc` break does not touch any API
  rustEZ uses.

### Notes

- `RouteEngine.status` of `"Testing"` on vSRX is **genuine device output**, not a
  parse artifact, and is deliberately passed through unmodified.
- The vSRX fixture backing these fixes is a verbatim capture from Junos
  24.4R1.9.

## [0.12.1] — 2026-07-02

### Security

- **Upgraded `quick-xml` `0.37` → `0.41`** — closes **RUSTSEC-2026-0194**
  (quadratic duplicate-attribute-name scan) and **RUSTSEC-2026-0195**
  (unbounded namespace-declaration allocation / memory-exhaustion DoS). Both
  are reachable on the fact-parsing path, which decodes device-supplied XML.

### Fixed

- **Fact parsers no longer truncate values containing XML entities.** Since
  quick-xml 0.38, entity references (`&amp;`, `&lt;`, `&#38;`, …) stream as
  separate `Event::GeneralRef` events instead of arriving inside `Text`. The
  four fact-parser reader loops (`facts/mod.rs`, `chassis.rs`, `software.rs`,
  `routing_engine.rs`) now accumulate `Text` + resolve `GeneralRef` and flush
  on the closing tag, so a Junos value such as a description or config
  fragment containing `&`/`<`/`>` round-trips correctly. Added entity
  round-trip regression tests. `unwrap_multi_re` keeps entities verbatim in
  reconstructed per-RE XML (and now escapes reconstructed attribute-value
  quotes) so downstream re-parsing stays well-formed.

### Changed

- Bumped `rustnetconf` dependency to `0.12.3` (pulls its own quick-xml 0.41
  fix for the same advisories).
- **MSRV raised to 1.79** (required by quick-xml ≥ 0.40).

## [0.12.0] — 2026-05-18

### Added

- **`HostKeyVerification::KnownHosts(PathBuf)` re-exported** — surfaces the new
  variant from `rustnetconf 0.12`. Callers can now point at an OpenSSH
  `known_hosts` file instead of pinning a single fingerprint.
- **Python `Device(host_key_known_hosts="...")`** — new constructor keyword
  argument on `rustez.Device` mapping to `HostKeyVerification::KnownHosts`.
  Mutually exclusive with `host_key_fingerprint` (raises `ValueError`).

### Changed

- Bumped `rustnetconf` dependency to `0.12`. Additive only for rustez — no
  source-level breakage. The 0.12 release added `KnownHosts(PathBuf)` to
  `HostKeyVerification` and a `host_key_verification` field on the pool
  `DeviceConfig` struct (rustez does not use the pool API).

### Fixed

- **Stale doc comments** on `DeviceBuilder::host_key_verification` (Rust) and
  `Device.__init__` (Python) — both incorrectly claimed the default policy
  was `AcceptAll`. Since `rustnetconf 0.11` the default has been `RejectAll`
  (fail-closed); the docs now reflect this.

[0.15.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.14.3...v0.15.0
[0.14.3]: https://github.com/fastrevmd-lab/rustez/compare/v0.14.2...v0.14.3
[0.14.2]: https://github.com/fastrevmd-lab/rustez/compare/v0.14.1...v0.14.2
[0.14.1]: https://github.com/fastrevmd-lab/rustez/compare/v0.14.0...v0.14.1
[0.14.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.13.1...v0.14.0
[0.13.1]: https://github.com/fastrevmd-lab/rustez/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.12.1...v0.13.0
[0.12.1]: https://github.com/fastrevmd-lab/rustez/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.11.0...v0.12.0

## [0.11.0] — 2026-05-18

### Changed

- **BREAKING:** Bumped `rustnetconf` dependency to `0.11`. The underlying SSH host-key verification default changed from `AcceptAll` to `RejectAll` (fail-closed). Callers that previously connected without setting a policy will now get a host-key rejection error at connect time.
  - **Migration:** Pin a fingerprint with `DeviceBuilder::host_key_verification(HostKeyVerification::Fingerprint(...))` (recommended), or explicitly opt back into the old behavior with `DeviceBuilder::host_key_verification(HostKeyVerification::AcceptAll)` for lab/test use.
  - **Python:** Pass `host_key_fingerprint="..."` to `Device(...)` to pin, or use `HostKeyVerification` directly via the native bindings.
- Integration test harness (`vsrx_builder` in `tests/integration_vsrx.rs`) updated to explicitly request `HostKeyVerification::AcceptAll` since the lab vSRX devices are known-good.

[0.11.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.10.0...v0.11.0

## [0.10.0] — 2026-05-06

### Fixed

- **PyDevice config methods bypass timeout protection** (PR #19) — `config_diff`, `config_commit`, and `config_rollback` in the Python bindings now route through `ConfigManager` instead of calling `client_mut()` directly, restoring per-RPC timeout wrapping.
- **`parse_cli_output` dead code** — removed unreachable duplicate `find("<output>")` block.
- **`validate_xml_name` accepts invalid names** — now rejects names starting with digits or hyphens per the XML specification.
- **Python `Config.commit(comment=...)` silently ignored** — wired through to native `commit_with_comment`.
- **Python `Config.diff(rb_id=N)` silently ignored** — `rb_id` parameter now passed to native layer.
- **`classify_error` misclassification** — reordered pattern matching; config-load errors now detected before generic timeout/connect checks; removed overly broad `"config"`/`"load"` catch-all.
- **Facts parsers silently swallow XML errors** — all four parsers now emit `tracing::warn!` on parse failures.

### Added

- **`Device::is_config_db_open()`** — public accessor for config database open state.
- **`cargo audit` in CI** — dependency vulnerability scanning runs on every PR, with `RUSTSEC-2023-0071` (rsa timing side-channel) ignored until upstream fix.
- **`cargo clippy` for `rustez-py`** in CI — previously only linted the core crate.
- **Least-privilege CI permissions** — `permissions: contents: read` added to CI workflow.

### Changed

- Bumped `rustnetconf` dependency to `0.10` — gains credential zeroization, XML fragment validation, built-in RPC timeout support, ProxyCommand shell injection fix, and max read buffer limit.
- Synced `rustez-py` version to match core crate.
- Removed unused `to_netconf_err` function from Python bindings.

[0.10.0]: https://github.com/fastrevmd-lab/rustez/compare/v0.8.4...v0.10.0

## 0.9.0 — 2026-05-04

<!-- Unlinked: v0.9.0 was never tagged in this repository. The 0.9.0 changes
     are contained in the v0.8.4...v0.10.0 range linked from 0.10.0 above. -->

### Added

- **SSH connectivity options** (PR #18) surfaced from rustnetconf 0.9:
  - `DeviceBuilder::jump_hosts(Vec<JumpHostConfig>)` — multi-hop bastion chain (OpenSSH `ProxyJump`).
  - `DeviceBuilder::proxy_command(&str)` — OpenSSH-style `ProxyCommand` with `%h` / `%p` substitution.
  - `Device::connect_via_ssh_config(alias)` and `Device::connect_via_ssh_config_at(path, alias)` — resolve a `Host` alias from `~/.ssh/config` (or an explicit path) into a populated `DeviceBuilder`. Subsequent builder calls override resolved values.
  - Re-exports `JumpHostConfig`, `SshConfigError`, `SshConfigFile`, and `ResolvedHost` from the crate root.
  - New `RustEzError::SshConfig` variant (mapped via `#[from]`) so callers don't need to import rustnetconf.
- **`ConfigManager::commit_with_comment`** (PR #16) — commit with an attached commit log comment.
- **`Serialize` derive** on `Facts`, `Personality`, and `RouteEngine` (PR #15) — enables direct JSON / serde output of gathered device facts.

### Changed

- Bumped `rustnetconf` dependency to `0.9`.

