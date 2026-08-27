//! Configuration management for Junos devices.

use std::time::Duration;

use quick_xml::escape::escape;
use rustnetconf::rpc::RpcErrorInfo;
use rustnetconf::{Client, Datastore, LoadAction, LoadFormat, OpenConfigurationMode};

/// XML namespace prefix used by rustnetconf for NETCONF RPCs.
const NC: &str = "nc:";

use crate::error::RustEzError;

/// Transient config helper returned by [`Device::config()`](crate::Device::config).
///
/// On chassis-clustered devices, [`load()`](Self::load) automatically opens
/// a private configuration database before loading, and [`unlock()`](Self::unlock)
/// closes it. Use [`open_configuration()`](Self::open_configuration) for
/// explicit control (e.g., exclusive mode).
pub struct ConfigManager<'a> {
    client: &'a mut Client,
    timeout: Duration,
    config_db_open: &'a mut bool,
}

/// The format/payload for a configuration load operation.
#[derive(Debug, Clone)]
pub enum ConfigPayload {
    /// Raw XML config elements.
    ///
    /// # Warning
    /// Content is embedded directly without escaping. Callers must not
    /// pass untrusted input — use [`Text`] or [`Set`] for user-provided config.
    Xml(String),
    /// Junos text format (curly brace).
    Text(String),
    /// "set" commands.
    Set(String),
}

impl<'a> ConfigManager<'a> {
    pub(crate) fn new(
        client: &'a mut Client,
        timeout: Duration,
        config_db_open: &'a mut bool,
    ) -> Self {
        Self {
            client,
            timeout,
            config_db_open,
        }
    }

    /// Lock the candidate datastore.
    pub async fn lock(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.lock(Datastore::Candidate)).await
    }

    /// Unlock the candidate datastore.
    ///
    /// If a private/exclusive configuration database was auto-opened,
    /// it is closed before unlocking.
    pub async fn unlock(&mut self) -> Result<(), RustEzError> {
        if *self.config_db_open {
            let timeout = self.timeout;
            timed(timeout, self.client.close_configuration()).await?;
            *self.config_db_open = false;
        }
        let timeout = self.timeout;
        timed(timeout, self.client.unlock(Datastore::Candidate)).await
    }

    /// Load configuration into the candidate datastore.
    ///
    /// On chassis-clustered devices, automatically opens a private
    /// configuration database if one is not already open. If the load
    /// fails, a database opened by this call is best-effort closed before
    /// returning the error. A database opened explicitly by the caller
    /// before this call is left alone.
    pub async fn load(&mut self, payload: ConfigPayload) -> Result<String, RustEzError> {
        let opened_here = self.auto_open_if_needed().await?;

        let (action, format, config) = payload_to_load_args(&payload);
        let timeout = self.timeout;
        let result = timed(
            timeout,
            self.client.load_configuration(action, format, &config),
        )
        .await;
        if result.is_err() {
            self.close_auto_opened_on_error(opened_here).await;
        }
        result
    }

    /// Load configuration with an explicit action (merge, replace, override, update).
    ///
    /// Use this when you need an action other than the default (merge for
    /// text/xml, set for set commands). See [`load()`](Self::load) for the
    /// auto-open and error-cleanup behavior.
    pub async fn load_with_action(
        &mut self,
        payload: ConfigPayload,
        action: LoadAction,
    ) -> Result<String, RustEzError> {
        let opened_here = self.auto_open_if_needed().await?;

        let (_default_action, format, config) = payload_to_load_args(&payload);
        let timeout = self.timeout;
        let result = timed(
            timeout,
            self.client.load_configuration(action, format, &config),
        )
        .await;
        if result.is_err() {
            self.close_auto_opened_on_error(opened_here).await;
        }
        result
    }

    /// Load configuration and return any warnings from the device.
    ///
    /// Warnings are non-fatal messages (severity="warning") that the device
    /// returns alongside a successful load. See [`load()`](Self::load) for
    /// the auto-open and error-cleanup behavior.
    ///
    /// Marks the candidate dirty so `close()` cleans up after this session.
    /// The mark is applied by rustnetconf's atomic candidate-change path, which
    /// validates and preflights before marking — a fragment that never reaches
    /// the device leaves no mark behind.
    pub async fn load_with_warnings(
        &mut self,
        payload: ConfigPayload,
    ) -> Result<(String, Vec<RpcErrorInfo>), RustEzError> {
        // rpc_candidate_change_with_warnings (rustnetconf 0.14.1) validates the
        // fragment, runs the send preflight, marks the candidate, and only then
        // writes — atomically. It replaces a hand-rolled validate-then-mark that
        // could not reach the private keepalive/ensure_established checks, so a
        // dead session could still leave a false mark for close() to act on.
        // Do not reintroduce a separate mark_candidate_dirty() call here: the
        // whole point is that nothing marks outside this one atomic path.
        let xml = build_load_xml(&payload);
        let opened_here = self.auto_open_if_needed().await?;

        let timeout = self.timeout;
        let result = timed(
            timeout,
            self.client.rpc_candidate_change_with_warnings(&xml),
        )
        .await;
        if result.is_err() {
            self.close_auto_opened_on_error(opened_here).await;
        }
        result
    }

    /// Show the candidate diff against the running configuration.
    ///
    /// Returns `Some(diff)` if there are changes, `None` if clean.
    /// Equivalent to [`diff_against(0)`](Self::diff_against).
    pub async fn diff(&mut self) -> Result<Option<String>, RustEzError> {
        self.diff_against(0).await
    }

    /// Show the candidate diff against a specific rollback ID.
    ///
    /// `rb_id` selects which historical configuration to compare against:
    /// `0` is the running configuration, `1` is the previous commit, and
    /// so on up to the platform-specific rollback retention limit
    /// (typically 49 on Junos).
    ///
    /// Returns `Some(diff)` if the two configurations differ, `None` if
    /// they are identical.
    ///
    /// ```rust,no_run
    /// # use rustez::Device;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut dev = Device::connect("10.0.0.1").open().await?;
    /// let mut cfg = dev.config()?;
    /// let diff_vs_prev_commit = cfg.diff_against(1).await?;
    /// # Ok(()) }
    /// ```
    pub async fn diff_against(&mut self, rb_id: u32) -> Result<Option<String>, RustEzError> {
        let timeout = self.timeout;
        let response: String = timed(timeout, self.client.get_configuration_compare(rb_id)).await?;

        let diff = parse_configuration_output(&response);
        if diff.is_empty() {
            Ok(None)
        } else {
            Ok(Some(diff))
        }
    }

    /// Commit the candidate configuration.
    ///
    /// Uses the Junos-native `<commit-configuration/>` RPC, which works
    /// correctly with private and exclusive configuration databases.
    pub async fn commit(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.commit_configuration()).await
    }

    /// Commit the candidate configuration with a log comment.
    ///
    /// Sends the Junos-native `<commit-configuration><log>…</log></commit-configuration>`
    /// RPC, attaching a comment visible in `show system commit`. Any string is
    /// safe to pass, including untrusted input — rustnetconf XML-escapes the
    /// comment, so callers must **not** pre-escape it.
    ///
    /// Like [`commit()`](Self::commit), this clears the candidate-dirty flag on
    /// success, so a session that commits with a comment does not discard at
    /// close.
    pub async fn commit_with_comment(&mut self, comment: &str) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.commit_configuration_with_log(comment)).await
    }

    /// Validate the candidate configuration without committing.
    pub async fn commit_check(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.validate(Datastore::Candidate)).await
    }

    /// Confirmed commit with automatic rollback after `seconds`.
    pub async fn commit_confirmed(&mut self, seconds: u32) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.confirmed_commit(seconds)).await
    }

    /// Rollback to a previous configuration.
    pub async fn rollback(&mut self, id: u32) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.rollback_configuration(id)).await
    }

    /// Open a private or exclusive configuration database explicitly.
    ///
    /// Call this before [`load()`](Self::load) if you need exclusive mode.
    /// For private mode, `load()` handles this automatically on clustered devices.
    pub async fn open_configuration(
        &mut self,
        mode: OpenConfigurationMode,
    ) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.open_configuration(mode)).await?;
        *self.config_db_open = true;
        Ok(())
    }

    /// Close a previously opened configuration database.
    ///
    /// No-op if no configuration database is open.
    pub async fn close_configuration(&mut self) -> Result<(), RustEzError> {
        if !*self.config_db_open {
            return Ok(());
        }
        let timeout = self.timeout;
        timed(timeout, self.client.close_configuration()).await?;
        *self.config_db_open = false;
        Ok(())
    }

    /// Auto-open a private configuration database if the device requires it.
    ///
    /// Returns `true` when this call opened the database (and is therefore
    /// responsible for cleanup on a subsequent error), `false` when it was
    /// already open or the device does not require an open database.
    async fn auto_open_if_needed(&mut self) -> Result<bool, RustEzError> {
        if self.client.requires_open_configuration() && !*self.config_db_open {
            self.open_configuration(OpenConfigurationMode::Private)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Best-effort close a configuration database that this call opened.
    ///
    /// No-op when `opened_here` is `false` — sessions opened by the caller
    /// before the failing load are preserved. Errors from `close_configuration`
    /// are swallowed so the original load error reaches the caller; the
    /// `config_db_open` flag is reset only when close actually succeeds.
    async fn close_auto_opened_on_error(&mut self, opened_here: bool) {
        if !opened_here {
            return;
        }
        let timeout = self.timeout;
        if timed(timeout, self.client.close_configuration())
            .await
            .is_ok()
        {
            *self.config_db_open = false;
        }
    }
}

/// Run an async future with a timeout, converting to RustEzError.
async fn timed<T>(
    timeout: Duration,
    future: impl std::future::Future<Output = Result<T, rustnetconf::NetconfError>>,
) -> Result<T, RustEzError> {
    match tokio::time::timeout(timeout, future).await {
        Ok(inner) => Ok(inner?),
        Err(_) => Err(RustEzError::Timeout(format!(
            "config operation timed out after {timeout:?}"
        ))),
    }
}

/// Map a `ConfigPayload` to `(LoadAction, LoadFormat, config_string)`.
///
/// The config string is XML-escaped for `Text` and `Set` payloads.
fn payload_to_load_args(payload: &ConfigPayload) -> (LoadAction, LoadFormat, String) {
    match payload {
        ConfigPayload::Xml(xml) => (LoadAction::Merge, LoadFormat::Xml, xml.clone()),
        ConfigPayload::Text(text) => (LoadAction::Merge, LoadFormat::Text, text.clone()),
        ConfigPayload::Set(set_cmds) => (LoadAction::Set, LoadFormat::Text, set_cmds.clone()),
    }
}

/// Build the `<load-configuration>` XML for a given payload.
///
/// Used by `load_with_warnings()`, which builds the fragment itself because the
/// typed `load_configuration()` does not return warnings. The fragment goes to
/// `rpc_candidate_change_with_warnings()`, which validates it before sending.
/// Uses `nc:` namespace prefix to match rustnetconf's RPC envelope.
/// `Text` and `Set` payloads are XML-escaped to prevent injection.
/// `Xml` is passed through raw since it's explicitly raw XML by design.
fn build_load_xml(payload: &ConfigPayload) -> String {
    match payload {
        ConfigPayload::Xml(xml) => {
            format!("<{NC}load-configuration>{xml}</{NC}load-configuration>")
        }
        ConfigPayload::Text(text) => {
            let escaped = escape(text);
            format!(
                r#"<{NC}load-configuration format="text"><{NC}configuration-text>{escaped}</{NC}configuration-text></{NC}load-configuration>"#
            )
        }
        ConfigPayload::Set(set_cmds) => {
            let escaped = escape(set_cmds);
            format!(
                r#"<{NC}load-configuration action="set" format="text"><{NC}configuration-set>{escaped}</{NC}configuration-set></{NC}load-configuration>"#
            )
        }
    }
}

/// Extract text from `<configuration-output>` tags, or return trimmed response.
fn parse_configuration_output(xml: &str) -> String {
    if let Some(start) = xml.find("<configuration-output>") {
        let content_start = start + "<configuration-output>".len();
        if let Some(end) = xml[content_start..].find("</configuration-output>") {
            return xml[content_start..content_start + end].trim().to_string();
        }
    }
    xml.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_load_xml_xml_payload() {
        let payload =
            ConfigPayload::Xml("<system><host-name>test</host-name></system>".to_string());
        let xml = build_load_xml(&payload);
        assert_eq!(
            xml,
            "<nc:load-configuration><system><host-name>test</host-name></system></nc:load-configuration>"
        );
    }

    #[test]
    fn test_build_load_xml_text_payload() {
        let payload = ConfigPayload::Text("system { host-name test; }".to_string());
        let xml = build_load_xml(&payload);
        assert!(xml.contains(r#"format="text""#));
        assert!(xml
            .contains("<nc:configuration-text>system { host-name test; }</nc:configuration-text>"));
    }

    #[test]
    fn test_build_load_xml_set_payload() {
        let payload = ConfigPayload::Set("set system host-name test".to_string());
        let xml = build_load_xml(&payload);
        assert!(xml.contains(r#"action="set""#));
        assert!(xml.contains(r#"format="text""#));
        assert!(
            xml.contains("<nc:configuration-set>set system host-name test</nc:configuration-set>")
        );
    }

    #[test]
    fn test_build_load_xml_text_escapes_xml() {
        let payload = ConfigPayload::Text("</nc:configuration-text><delete-config/>".to_string());
        let xml = build_load_xml(&payload);
        assert!(!xml.contains("<delete-config/>"));
        assert!(xml.contains("&lt;delete-config/&gt;"));
    }

    #[test]
    fn test_build_load_xml_set_escapes_xml() {
        let payload = ConfigPayload::Set("</nc:configuration-set><evil/>".to_string());
        let xml = build_load_xml(&payload);
        assert!(!xml.contains("<evil/>"));
        assert!(xml.contains("&lt;evil/&gt;"));
    }

    #[test]
    fn test_parse_diff_with_content() {
        let response = r#"<configuration-output>
[edit system]
-  host-name old;
+  host-name new;
</configuration-output>"#;
        let diff = parse_configuration_output(response);
        assert!(diff.contains("host-name old"));
        assert!(diff.contains("host-name new"));
    }

    #[test]
    fn test_parse_diff_empty() {
        let response = "<configuration-output></configuration-output>";
        let diff = parse_configuration_output(response);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_payload_to_load_args_xml() {
        let payload = ConfigPayload::Xml("<system/>".to_string());
        let (action, format, config) = payload_to_load_args(&payload);
        assert_eq!(action, LoadAction::Merge);
        assert_eq!(format, LoadFormat::Xml);
        assert_eq!(config, "<system/>");
    }

    #[test]
    fn test_payload_to_load_args_text() {
        let payload = ConfigPayload::Text("system { host-name foo; }".to_string());
        let (action, format, _config) = payload_to_load_args(&payload);
        assert_eq!(action, LoadAction::Merge);
        assert_eq!(format, LoadFormat::Text);
    }

    #[test]
    fn test_payload_to_load_args_set() {
        let payload = ConfigPayload::Set("set system host-name foo".to_string());
        let (action, format, _config) = payload_to_load_args(&payload);
        assert_eq!(action, LoadAction::Set);
        assert_eq!(format, LoadFormat::Text);
    }
}
