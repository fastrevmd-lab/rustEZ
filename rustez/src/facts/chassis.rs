//! Parser for `<chassis-inventory>` RPC responses.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Parse the serial number from `<chassis-inventory>` XML.
///
/// Extracts `<chassis><serial-number>` from the response.
pub(crate) fn parse_serial_number(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut in_chassis = false;
    let mut in_serial = false;
    let mut depth: u32 = 0;
    let mut serial = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref tag)) => {
                let local = tag.local_name();
                let name = local.as_ref();
                match name {
                    "chassis" if !in_chassis => {
                        in_chassis = true;
                        depth = 1;
                    }
                    "serial-number" if in_chassis && depth == 1 => {
                        in_serial = true;
                    }
                    _ if in_chassis => {
                        depth += 1;
                    }
                    _ => {}
                }
            }
            // Since quick-xml 0.38, a serial containing an entity (`&`) splits
            // across Text/GeneralRef events, so accumulate and flush on the
            // closing tag rather than returning on the first Text event.
            Ok(Event::Text(ref text)) if in_serial => {
                serial.push_str(text);
            }
            Ok(Event::GeneralRef(ref entity)) if in_serial => {
                if let Some(resolved) = super::xml_entity::resolve_entity_ref(entity) {
                    serial.push_str(&resolved);
                }
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = local.as_ref();
                if name == "serial-number" {
                    in_serial = false;
                    let trimmed = serial.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                    serial.clear();
                } else if name == "chassis" {
                    in_chassis = false;
                } else if in_chassis {
                    depth = depth.saturating_sub(1);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                tracing::warn!("XML parse error in chassis facts: {err}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_serial_number() {
        let xml = r#"<chassis-inventory>
  <chassis>
    <name>Chassis</name>
    <serial-number>CY0216AF0077</serial-number>
    <description>vSRX</description>
    <chassis-module>
      <name>FPC 0</name>
      <serial-number>MODULE-SN</serial-number>
    </chassis-module>
  </chassis>
</chassis-inventory>"#;

        let serial = parse_serial_number(xml);
        assert_eq!(serial.as_deref(), Some("CY0216AF0077"));
    }

    #[test]
    fn test_parse_serial_number_with_entities() {
        // quick-xml 0.38+ streams entities as separate GeneralRef events;
        // a serial containing `&`/`<` must be stitched back, not truncated.
        let xml = r#"<chassis-inventory>
  <chassis>
    <name>Chassis</name>
    <serial-number>CY&amp;02&lt;16</serial-number>
  </chassis>
</chassis-inventory>"#;

        let serial = parse_serial_number(xml);
        assert_eq!(serial.as_deref(), Some("CY&02<16"));
    }

    #[test]
    fn test_parse_serial_number_missing() {
        let xml = r#"<chassis-inventory>
  <chassis>
    <name>Chassis</name>
    <description>vSRX</description>
  </chassis>
</chassis-inventory>"#;

        let serial = parse_serial_number(xml);
        assert!(serial.is_none());
    }
}
