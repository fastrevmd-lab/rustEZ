//! Configuration management for Junos devices.

use std::time::Duration;

use rustnetconf::{Client, Datastore};

use crate::error::RustEzError;

/// Transient config helper returned by [`Device::config()`](crate::Device::config).
pub struct ConfigManager<'a> {
    client: &'a mut Client,
    timeout: Duration,
}

/// The format/payload for a configuration load operation.
#[derive(Debug, Clone)]
pub enum ConfigPayload {
    /// Raw XML config elements.
    Xml(String),
    /// Junos text format (curly brace).
    Text(String),
    /// "set" commands.
    Set(String),
}

impl<'a> ConfigManager<'a> {
    pub(crate) fn new(client: &'a mut Client, timeout: Duration) -> Self {
        Self { client, timeout }
    }

    /// Lock the candidate datastore.
    pub async fn lock(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.lock(Datastore::Candidate)).await
    }

    /// Unlock the candidate datastore.
    pub async fn unlock(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.unlock(Datastore::Candidate)).await
    }

    /// Load configuration into the candidate datastore.
    pub async fn load(&mut self, payload: ConfigPayload) -> Result<String, RustEzError> {
        let xml = build_load_xml(&payload);
        let timeout = self.timeout;
        timed(timeout, self.client.rpc(&xml)).await
    }

    /// Show the candidate diff (uncommitted changes).
    ///
    /// Returns `Some(diff)` if there are changes, `None` if clean.
    pub async fn diff(&mut self) -> Result<Option<String>, RustEzError> {
        let xml = r#"<get-configuration compare="rollback" rollback="0" format="text"/>"#;
        let timeout = self.timeout;
        let response: String = timed(timeout, self.client.rpc(xml)).await?;

        let diff = parse_configuration_output(&response);
        if diff.is_empty() {
            Ok(None)
        } else {
            Ok(Some(diff))
        }
    }

    /// Commit the candidate configuration.
    pub async fn commit(&mut self) -> Result<(), RustEzError> {
        let timeout = self.timeout;
        timed(timeout, self.client.commit()).await
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
    pub async fn rollback(&mut self, id: u32) -> Result<String, RustEzError> {
        let xml = format!(r#"<load-configuration rollback="{id}"/>"#);
        let timeout = self.timeout;
        timed(timeout, self.client.rpc(&xml)).await
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

/// Build the `<load-configuration>` XML for a given payload.
fn build_load_xml(payload: &ConfigPayload) -> String {
    match payload {
        ConfigPayload::Xml(xml) => {
            format!("<load-configuration>{xml}</load-configuration>")
        }
        ConfigPayload::Text(text) => {
            format!(
                r#"<load-configuration format="text"><configuration-text>{text}</configuration-text></load-configuration>"#
            )
        }
        ConfigPayload::Set(set_cmds) => {
            format!(
                r#"<load-configuration format="set"><configuration-set>{set_cmds}</configuration-set></load-configuration>"#
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
            "<load-configuration><system><host-name>test</host-name></system></load-configuration>"
        );
    }

    #[test]
    fn test_build_load_xml_text_payload() {
        let payload = ConfigPayload::Text("system { host-name test; }".to_string());
        let xml = build_load_xml(&payload);
        assert!(xml.contains(r#"format="text""#));
        assert!(xml.contains("<configuration-text>system { host-name test; }</configuration-text>"));
    }

    #[test]
    fn test_build_load_xml_set_payload() {
        let payload = ConfigPayload::Set("set system host-name test".to_string());
        let xml = build_load_xml(&payload);
        assert!(xml.contains(r#"format="set""#));
        assert!(xml.contains("<configuration-set>set system host-name test</configuration-set>"));
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
}
