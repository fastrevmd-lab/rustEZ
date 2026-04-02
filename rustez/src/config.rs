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
    /// configuration database if one is not already open.
    pub async fn load(&mut self, payload: ConfigPayload) -> Result<String, RustEzError> {
        self.auto_open_if_needed().await?;

        let (action, format, config) = payload_to_load_args(&payload);
        let timeout = self.timeout;
        timed(
            timeout,
            self.client.load_configuration(action, format, &config),
        )
        .await
    }

    /// Load configuration with an explicit action (merge, replace, override, update).
    ///
    /// Use this when you need an action other than the default (merge for
    /// text/xml, set for set commands).
    pub async fn load_with_action(
        &mut self,
        payload: ConfigPayload,
        action: LoadAction,
    ) -> Result<String, RustEzError> {
        self.auto_open_if_needed().await?;

        let (_default_action, format, config) = payload_to_load_args(&payload);
        let timeout = self.timeout;
        timed(
            timeout,
            self.client.load_configuration(action, format, &config),
        )
        .await
    }

    /// Load configuration and return any warnings from the device.
    ///
    /// Warnings are non-fatal messages (severity="warning") that the device
    /// returns alongside a successful load.
    pub async fn load_with_warnings(
        &mut self,
        payload: ConfigPayload,
    ) -> Result<(String, Vec<RpcErrorInfo>), RustEzError> {
        self.auto_open_if_needed().await?;

        let xml = build_load_xml(&payload);
        let timeout = self.timeout;
        timed(timeout, self.client.rpc_with_warnings(&xml)).await
    }

    /// Show the candidate diff (uncommitted changes).
    ///
    /// Returns `Some(diff)` if there are changes, `None` if clean.
    pub async fn diff(&mut self) -> Result<Option<String>, RustEzError> {
        let timeout = self.timeout;
        let response: String =
            timed(timeout, self.client.get_configuration_compare(0)).await?;

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
    async fn auto_open_if_needed(&mut self) -> Result<(), RustEzError> {
        if self.client.requires_open_configuration() && !*self.config_db_open {
            self.open_configuration(OpenConfigurationMode::Private)
                .await?;
        }
        Ok(())
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
        ConfigPayload::Text(text) => {
            (LoadAction::Merge, LoadFormat::Text, text.clone())
        }
        ConfigPayload::Set(set_cmds) => {
            (LoadAction::Set, LoadFormat::Text, set_cmds.clone())
        }
    }
}

/// Build the `<load-configuration>` XML for a given payload.
///
/// Used by `load_with_warnings()` which needs raw RPC for warning extraction.
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
        let payload = ConfigPayload::Xml("<system><host-name>test</host-name></system>".to_string());
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
        assert!(xml.contains("<nc:configuration-text>system { host-name test; }</nc:configuration-text>"));
    }

    #[test]
    fn test_build_load_xml_set_payload() {
        let payload = ConfigPayload::Set("set system host-name test".to_string());
        let xml = build_load_xml(&payload);
        assert!(xml.contains(r#"action="set""#));
        assert!(xml.contains(r#"format="text""#));
        assert!(xml.contains("<nc:configuration-set>set system host-name test</nc:configuration-set>"));
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
