# Ignored `cargo audit` advisories

This file documents advisories explicitly ignored in CI. Each entry must
include the dependency path, reachability assessment, and review date.
Re-evaluate every entry on or before the listed `Review by` date.

CI references this file via the `cargo audit --ignore <ID>` flag in
`.github/workflows/ci.yml`. Keep the flags in sync with the list below.

**There are currently no active ignores**, and `cargo audit` runs unsuppressed.
Retired entries are kept below with the reason they were retired, so the history
of what was once accepted stays readable.

---

## RETIRED 2026-08-26 — RUSTSEC-2023-0071 — `rsa 0.10.0-rc.16` (Marvin Attack)

> **Retired because the crate left the dependency tree.** `russh` reworked its
> crypto dependencies and `rsa` is no longer reachable from `rustez` at any
> depth (`cargo tree -i rsa` matches nothing); `cargo audit` passes clean with
> no `--ignore`. The flag was removed from `.github/workflows/ci.yml`. A
> standing ignore for an absent crate is worse than none — it would silently
> suppress this advisory if the crate ever came back. The assessment below is
> preserved as written at the time it was active.

- **Advisory:** https://github.com/RustCrypto/RSA/issues/626
- **CVSS:** 3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:N/A:N (medium; network-reachable,
  high attack complexity, confidentiality impact only)
- **Patched version:** none available. The RustCrypto/RSA project has not
  shipped a constant-time fix; the rc.16 release-candidate inherits the same
  timing side-channel as the stable 0.9 line.
- **Dependency path** (from `cargo tree -i rsa`):
  ```
  rsa 0.10.0-rc.16
    └── internal-russh-forked-ssh-key 0.6.18+upstream-0.6.7
          └── russh 0.60.2
                └── rustnetconf 0.10.0
                      └── rustez 0.10.1
  ```
- **Reachability:** **REACHABLE.** russh uses `rsa` for SSH host-key
  verification and RSA client authentication. rustEZ connects to Junos
  devices over SSH and any of those connections may negotiate RSA host
  keys or RSA user-auth. An attacker who can observe many SSH handshakes
  to the same device could in theory mount a timing attack against the
  private key. Practical exploitability is bounded by the high attack
  complexity rating and the typical lab/admin network exposure of NETCONF.
- **Mitigations available to users:**
  1. Prefer ed25519 or ECDSA host keys and user keys over RSA.
  2. Restrict NETCONF reachability to trusted management networks.
  3. Pin device fingerprints via `Device::host_key_verification()` (RZ-SEC-001)
     to detect MITM independent of RSA-specific weaknesses.
- **Decision:** accept the risk and ignore in CI until upstream ships a fix.
- **Review by:** 2026-11-18 (six months from initial documentation).
- **First documented:** 2026-05-18.
