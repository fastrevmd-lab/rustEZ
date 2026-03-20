//! Junos device facts gathering.
//!
//! Gathers operational facts (hostname, model, version, serial, route engines)
//! from a Junos device via three sequential RPCs.

pub mod chassis;
pub mod personality;
pub mod routing_engine;
pub mod software;

use std::time::Duration;

use rustnetconf::Client;

use crate::error::RustEzError;
pub use personality::{detect_personality, Personality};
pub use routing_engine::RouteEngine;

/// Collected facts about a Junos device.
#[derive(Debug, Clone)]
pub struct Facts {
    /// Device hostname.
    pub hostname: String,
    /// Device model (e.g., "vSRX", "MX480").
    pub model: String,
    /// Junos version string.
    pub version: String,
    /// Chassis serial number.
    pub serial_number: String,
    /// Detected platform personality.
    pub personality: Personality,
    /// Route engine information (one per RE).
    pub route_engines: Vec<RouteEngine>,
    /// Index into `route_engines` for the master RE.
    pub master_re: Option<usize>,
    /// DNS domain name.
    pub domain: Option<String>,
    /// Fully qualified domain name.
    pub fqdn: Option<String>,
}

/// Gather facts from a connected Junos device.
///
/// Sends three RPCs sequentially, each wrapped in a per-RPC timeout:
/// 1. `<get-software-information/>` — hostname, model, version
/// 2. `<get-chassis-inventory/>` — serial number
/// 3. `<get-route-engine-information/>` — RE status
pub(crate) async fn gather_facts(
    client: &mut Client,
    timeout: Duration,
) -> Result<Facts, RustEzError> {
    // 1. Software information
    let sw_xml = rpc_with_timeout(client, "<get-software-information/>", timeout).await?;
    let sw_items = unwrap_multi_re(&sw_xml);
    // Use first RE's software info (or the only one for single-RE)
    let first_sw_xml = &sw_items[0].1;
    let sw_info = software::parse_software_info(first_sw_xml);

    let hostname = sw_info
        .hostname
        .unwrap_or_else(|| "unknown".to_string());
    let model = sw_info
        .model
        .unwrap_or_else(|| "unknown".to_string());
    let version = sw_info
        .version
        .unwrap_or_else(|| "unknown".to_string());

    // Derive FQDN/domain from hostname
    let (domain, fqdn) = if hostname.contains('.') {
        let parts: Vec<&str> = hostname.splitn(2, '.').collect();
        (Some(parts[1].to_string()), Some(hostname.clone()))
    } else {
        (None, None)
    };

    // 2. Chassis inventory
    let chassis_xml = rpc_with_timeout(client, "<get-chassis-inventory/>", timeout).await?;
    let chassis_items = unwrap_multi_re(&chassis_xml);
    let serial_number = chassis::parse_serial_number(&chassis_items[0].1)
        .unwrap_or_else(|| "unknown".to_string());

    // 3. Route engine information
    let re_xml =
        rpc_with_timeout(client, "<get-route-engine-information/>", timeout).await?;
    let re_items = unwrap_multi_re(&re_xml);

    let mut route_engines = Vec::new();
    for (_re_name, re_content) in &re_items {
        let mut engines = routing_engine::parse_route_engines(re_content);
        route_engines.append(&mut engines);
    }

    let master_re = routing_engine::find_master_re(&route_engines);
    let personality = detect_personality(&model);

    Ok(Facts {
        hostname,
        model,
        version,
        serial_number,
        personality,
        route_engines,
        master_re,
        domain,
        fqdn,
    })
}

/// Send an RPC with a per-RPC timeout.
async fn rpc_with_timeout(
    client: &mut Client,
    rpc_content: &str,
    timeout: Duration,
) -> Result<String, RustEzError> {
    let result = tokio::time::timeout(timeout, client.rpc(rpc_content)).await;
    match result {
        Ok(inner) => inner.map_err(RustEzError::from),
        Err(_) => Err(RustEzError::Timeout(format!(
            "facts RPC timed out after {timeout:?}: {rpc_content}"
        ))),
    }
}

/// Detect and unwrap `<multi-routing-engine-results>` XML wrapper.
///
/// If the XML contains a `<multi-routing-engine-results>` wrapper,
/// returns a `Vec<(Option<re_name>, inner_xml)>` with one entry per RE.
/// Otherwise returns a single-element Vec with `(None, original_xml)`.
pub fn unwrap_multi_re(xml: &str) -> Vec<(Option<String>, String)> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    // Quick check: if no multi-RE wrapper, return as-is
    if !xml.contains("multi-routing-engine-results") {
        return vec![(None, xml.to_string())];
    }

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut results = Vec::new();

    let mut in_item = false;
    let mut current_re_name: Option<String> = None;
    let mut item_depth: u32 = 0;
    let mut item_content = String::new();
    let mut in_re_name = false;
    let mut capturing = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if name == "multi-routing-engine-item" {
                    in_item = true;
                    current_re_name = None;
                    item_content.clear();
                    capturing = false;
                } else if in_item && name == "re-name" {
                    in_re_name = true;
                } else if in_item && !in_re_name && name != "multi-routing-engine-results" {
                    if !capturing {
                        capturing = true;
                        item_depth = 0;
                    }
                    if capturing {
                        item_depth += 1;
                        item_content.push('<');
                        item_content.push_str(name);
                        for attr in tag.attributes().flatten() {
                            item_content.push(' ');
                            item_content.push_str(
                                std::str::from_utf8(attr.key.as_ref()).unwrap_or(""),
                            );
                            item_content.push_str("=\"");
                            item_content.push_str(
                                &String::from_utf8_lossy(&attr.value),
                            );
                            item_content.push('"');
                        }
                        item_content.push('>');
                    }
                }
            }
            Ok(Event::Text(ref text)) => {
                let value = text.unescape().unwrap_or_default().to_string();
                if in_re_name {
                    current_re_name = Some(value);
                } else if capturing {
                    item_content.push_str(&value);
                }
            }
            Ok(Event::Empty(ref tag)) if capturing => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
                item_content.push('<');
                item_content.push_str(name);
                for attr in tag.attributes().flatten() {
                    item_content.push(' ');
                    item_content.push_str(
                        std::str::from_utf8(attr.key.as_ref()).unwrap_or(""),
                    );
                    item_content.push_str("=\"");
                    item_content.push_str(
                        &String::from_utf8_lossy(&attr.value),
                    );
                    item_content.push('"');
                }
                item_content.push_str("/>");
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");

                if name == "re-name" {
                    in_re_name = false;
                } else if name == "multi-routing-engine-item" {
                    in_item = false;
                    if !item_content.is_empty() {
                        results.push((current_re_name.take(), item_content.clone()));
                    }
                } else if capturing {
                    item_depth -= 1;
                    item_content.push_str("</");
                    item_content.push_str(name);
                    item_content.push('>');
                    if item_depth == 0 {
                        capturing = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    if results.is_empty() {
        vec![(None, xml.to_string())]
    } else {
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwrap_multi_re_with_wrapper() {
        let xml = r#"<multi-routing-engine-results>
  <multi-routing-engine-item>
    <re-name>node0</re-name>
    <software-information>
      <host-name>node0</host-name>
    </software-information>
  </multi-routing-engine-item>
  <multi-routing-engine-item>
    <re-name>node1</re-name>
    <software-information>
      <host-name>node1</host-name>
    </software-information>
  </multi-routing-engine-item>
</multi-routing-engine-results>"#;

        let items = unwrap_multi_re(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0.as_deref(), Some("node0"));
        assert!(items[0].1.contains("<software-information>"));
        assert!(items[0].1.contains("node0"));
        assert_eq!(items[1].0.as_deref(), Some("node1"));
        assert!(items[1].1.contains("node1"));
    }

    #[test]
    fn test_unwrap_multi_re_without_wrapper() {
        let xml = r#"<software-information>
  <host-name>vsrx1</host-name>
</software-information>"#;

        let items = unwrap_multi_re(xml);
        assert_eq!(items.len(), 1);
        assert!(items[0].0.is_none());
        assert_eq!(items[0].1, xml);
    }
}
