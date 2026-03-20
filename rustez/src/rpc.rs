//! RPC execution helpers for Junos devices.

use std::time::Duration;

use quick_xml::escape::escape;
use rustnetconf::Client;

use crate::error::RustEzError;

/// Transient RPC helper returned by [`Device::rpc()`](crate::Device::rpc).
pub struct RpcExecutor<'a> {
    client: &'a mut Client,
    timeout: Duration,
}

impl<'a> RpcExecutor<'a> {
    pub(crate) fn new(client: &'a mut Client, timeout: Duration) -> Self {
        Self { client, timeout }
    }

    /// Call a named RPC with key-value arguments.
    ///
    /// Underscores in `rpc_name` are converted to hyphens.
    /// Each `(key, value)` pair becomes a child XML element.
    pub async fn call(
        &mut self,
        rpc_name: &str,
        args: &[(&str, &str)],
    ) -> Result<String, RustEzError> {
        let xml = build_rpc_xml(rpc_name, args)?;
        self.call_xml(&xml).await
    }

    /// Send pre-built XML directly as an RPC.
    pub async fn call_xml(&mut self, xml: &str) -> Result<String, RustEzError> {
        let result = tokio::time::timeout(self.timeout, self.client.rpc(xml)).await;
        match result {
            Ok(inner) => Ok(inner?),
            Err(_) => Err(RustEzError::Timeout(format!(
                "RPC timed out after {:?}",
                self.timeout
            ))),
        }
    }

    /// Execute a CLI command on the device.
    ///
    /// Wraps the command in a `<command>` RPC element and parses
    /// the `<output>` from the response.
    pub async fn cli(&mut self, command: &str, format: &str) -> Result<String, RustEzError> {
        validate_xml_name(format)?;
        let escaped_command = escape(command);
        let xml = format!(r#"<command format="{format}">{escaped_command}</command>"#);
        let response = self.call_xml(&xml).await?;
        Ok(parse_cli_output(&response))
    }
}

/// Build RPC XML from a name and key-value arguments.
///
/// Underscores in the RPC name and argument keys are converted to hyphens.
/// Names and keys are validated to prevent XML injection. Values are
/// XML-escaped.
#[allow(clippy::result_large_err)]
pub fn build_rpc_xml(rpc_name: &str, args: &[(&str, &str)]) -> Result<String, RustEzError> {
    let hyphenated_name = rpc_name.replace('_', "-");
    validate_xml_name(&hyphenated_name)?;

    if args.is_empty() {
        return Ok(format!("<{hyphenated_name}/>"));
    }

    let mut xml = format!("<{hyphenated_name}>");
    for (key, value) in args {
        let hyphenated_key = key.replace('_', "-");
        validate_xml_name(&hyphenated_key)?;
        let escaped_value = escape(*value);
        xml.push_str(&format!("<{hyphenated_key}>{escaped_value}</{hyphenated_key}>"));
    }
    xml.push_str(&format!("</{hyphenated_name}>"));
    Ok(xml)
}

/// Validate that a string is safe for use as an XML element name or attribute value.
///
/// Allows alphanumeric characters, hyphens, underscores, and dots.
#[allow(clippy::result_large_err)]
fn validate_xml_name(name: &str) -> Result<(), RustEzError> {
    if name.is_empty() {
        return Err(RustEzError::Rpc("XML name cannot be empty".to_string()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err(RustEzError::Rpc(format!(
            "invalid XML name: contains disallowed characters: {name:?}"
        )));
    }
    Ok(())
}

/// Extract text content from `<output>` elements, or return the raw response.
fn parse_cli_output(xml: &str) -> String {
    // Simple extraction: find <output>...</output>
    if let Some(start) = xml.find("<output>") {
        let content_start = start + "<output>".len();
        if let Some(end) = xml[content_start..].find("</output>") {
            return xml[content_start..content_start + end].to_string();
        }
    }
    // If wrapped in <cli> with nested <output>
    if let Some(start) = xml.find("<output>") {
        let after = &xml[start + "<output>".len()..];
        if let Some(end) = after.find("</output>") {
            return after[..end].to_string();
        }
    }
    xml.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_underscore_to_hyphen_conversion() {
        let xml = build_rpc_xml("get_software_information", &[]).unwrap();
        assert_eq!(xml, "<get-software-information/>");
    }

    #[test]
    fn test_args_to_child_xml_elements() {
        let xml = build_rpc_xml(
            "get_interface_information",
            &[("interface_name", "ge-0/0/0"), ("terse", "")],
        )
        .unwrap();
        assert!(xml.starts_with("<get-interface-information>"));
        assert!(xml.contains("<interface-name>ge-0/0/0</interface-name>"));
        assert!(xml.contains("<terse></terse>"));
        assert!(xml.ends_with("</get-interface-information>"));
    }

    #[test]
    fn test_xml_injection_in_rpc_name_rejected() {
        let result = build_rpc_xml("foo><evil/><bar", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_injection_in_arg_key_rejected() {
        let result = build_rpc_xml("get-info", &[("name><evil/><x", "val")]);
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_injection_in_arg_value_escaped() {
        let xml = build_rpc_xml("get-info", &[("name", "<script>alert(1)</script>")]).unwrap();
        assert!(xml.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!xml.contains("<script>"));
    }

    #[test]
    fn test_parse_cli_output_with_output_tags() {
        let xml = "<output>Interface      Status\nge-0/0/0       up</output>";
        let result = parse_cli_output(xml);
        assert_eq!(result, "Interface      Status\nge-0/0/0       up");
    }

    #[test]
    fn test_parse_cli_output_without_tags() {
        let xml = "raw text response";
        let result = parse_cli_output(xml);
        assert_eq!(result, "raw text response");
    }
}
