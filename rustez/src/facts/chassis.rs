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

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
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
            Ok(Event::Text(ref text)) if in_serial => {
                let value = text.unescape().unwrap_or_default().trim().to_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if name == "serial-number" {
                    in_serial = false;
                } else if name == "chassis" {
                    in_chassis = false;
                } else if in_chassis {
                    depth -= 1;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
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
