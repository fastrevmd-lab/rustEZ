//! Parser for `<software-information>` RPC responses.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Parsed software information from a Junos device.
#[derive(Debug, Default)]
pub(crate) struct SoftwareInfo {
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub version: Option<String>,
}

/// Parse `<software-information>` XML into a `SoftwareInfo`.
///
/// Extracts `<host-name>`, `<product-model>`, and `<junos-version>`.
/// Falls back to parsing version from `<package-information><comment>` if
/// `<junos-version>` is not present.
pub(crate) fn parse_software_info(xml: &str) -> SoftwareInfo {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut info = SoftwareInfo::default();

    let mut current_element = String::new();
    let mut in_package_comment = false;
    let mut in_package_info = false;
    let mut text_buf = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref tag)) => {
                let local = tag.local_name();
                let name = local.as_ref().to_string();
                match name.as_str() {
                    "package-information" => in_package_info = true,
                    "comment" if in_package_info => in_package_comment = true,
                    _ => {}
                }
                current_element = name;
                // Reset the per-element text buffer at each Start so only the
                // element's own character data (not inter-element whitespace)
                // is captured.
                text_buf.clear();
            }
            // Accumulate character data across Text/GeneralRef events. Since
            // quick-xml 0.38, entity refs (`&amp;`, `&#38;`, …) arrive as
            // separate GeneralRef events rather than inside Text; the value is
            // flushed and dispatched on the element's closing tag.
            Ok(Event::Text(ref text)) => {
                text_buf.push_str(text);
            }
            Ok(Event::GeneralRef(ref entity)) => {
                if let Some(resolved) = super::xml_entity::resolve_entity_ref(entity) {
                    text_buf.push_str(&resolved);
                }
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = local.as_ref();
                let value = std::mem::take(&mut text_buf);
                match name {
                    "host-name" => info.hostname = Some(value),
                    "product-model" => info.model = Some(value),
                    "junos-version" => info.version = Some(value),
                    "comment" if in_package_comment && info.version.is_none() => {
                        // Fallback: parse version from comment like
                        // "JUNOS Software Release [21.4R3.15]"
                        if let Some(version) = extract_version_from_comment(&value) {
                            info.version = Some(version);
                        }
                    }
                    _ => {}
                }
                match name {
                    "package-information" => {
                        in_package_info = false;
                        in_package_comment = false;
                    }
                    "comment" => in_package_comment = false,
                    _ => {}
                }
                current_element.clear();
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                tracing::warn!("XML parse error in software facts: {err}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    info
}

/// Extract a version string from a JUNOS comment like
/// `"JUNOS Software Release [21.4R3.15]"`.
fn extract_version_from_comment(comment: &str) -> Option<String> {
    // Look for content in square brackets
    if let Some(start) = comment.find('[') {
        if let Some(end) = comment[start..].find(']') {
            return Some(comment[start + 1..start + end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_re_software_info() {
        let xml = r#"<software-information>
  <host-name>vsrx1</host-name>
  <product-model>vSRX</product-model>
  <product-name>vsrx</product-name>
  <junos-version>21.4R3.15</junos-version>
  <package-information>
    <name>junos</name>
    <comment>JUNOS Software Release [21.4R3.15]</comment>
  </package-information>
</software-information>"#;

        let info = parse_software_info(xml);
        assert_eq!(info.hostname.as_deref(), Some("vsrx1"));
        assert_eq!(info.model.as_deref(), Some("vSRX"));
        assert_eq!(info.version.as_deref(), Some("21.4R3.15"));
    }

    #[test]
    fn test_multi_re_software_info_per_re() {
        // When called per-RE (after unwrap_multi_re), each RE's
        // <software-information> is parsed individually
        let xml = r#"<software-information>
  <host-name>mx-node0</host-name>
  <product-model>MX480</product-model>
  <junos-version>22.2R1.9</junos-version>
</software-information>"#;

        let info = parse_software_info(xml);
        assert_eq!(info.hostname.as_deref(), Some("mx-node0"));
        assert_eq!(info.model.as_deref(), Some("MX480"));
        assert_eq!(info.version.as_deref(), Some("22.2R1.9"));
    }

    #[test]
    fn test_missing_junos_version_fallback_to_comment() {
        let xml = r#"<software-information>
  <host-name>old-router</host-name>
  <product-model>M320</product-model>
  <package-information>
    <name>junos</name>
    <comment>JUNOS Base OS Software Suite [12.3R12.4]</comment>
  </package-information>
</software-information>"#;

        let info = parse_software_info(xml);
        assert_eq!(info.hostname.as_deref(), Some("old-router"));
        assert_eq!(info.version.as_deref(), Some("12.3R12.4"));
    }

    #[test]
    fn test_software_info_with_entities() {
        // A host-name / comment containing XML entities must round-trip:
        // quick-xml 0.38+ splits entity refs into GeneralRef events, so a
        // naive decode would truncate the value at the first `&`.
        let xml = r#"<software-information>
  <host-name>a&amp;b&lt;c</host-name>
  <product-model>vSRX</product-model>
  <junos-version>21.4R3.15</junos-version>
</software-information>"#;

        let info = parse_software_info(xml);
        assert_eq!(info.hostname.as_deref(), Some("a&b<c"));
        assert_eq!(info.model.as_deref(), Some("vSRX"));
    }

    #[test]
    fn test_missing_fields_graceful_defaults() {
        let xml = r#"<software-information>
  <product-name>vsrx</product-name>
</software-information>"#;

        let info = parse_software_info(xml);
        assert!(info.hostname.is_none());
        assert!(info.model.is_none());
        assert!(info.version.is_none());
    }
}
