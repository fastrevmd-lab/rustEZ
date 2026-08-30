//! Junos device facts gathering.
//!
//! Gathers operational facts (hostname, model, version, serial, route engines)
//! from a Junos device via three sequential RPCs.

pub mod chassis;
pub mod personality;
pub mod routing_engine;
pub mod software;
mod xml_entity;

use std::time::Duration;

use rustnetconf::Client;
use serde::Serialize;

use crate::error::RustEzError;
pub use personality::{detect_personality, Personality};
pub use routing_engine::RouteEngine;

/// Collected facts about a Junos device.
#[derive(Debug, Clone, Serialize)]
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
    /// Whether the device is part of a chassis cluster.
    pub is_cluster: bool,
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
    let is_cluster = sw_items.len() > 1;
    // Use first RE's software info (or the only one for single-RE)
    let first_sw_xml = &sw_items
        .first()
        .ok_or_else(|| RustEzError::Facts("empty software-information response".to_string()))?
        .1;
    let sw_info = software::parse_software_info(first_sw_xml);

    let hostname = sw_info.hostname.unwrap_or_else(|| "unknown".to_string());
    let model = sw_info.model.unwrap_or_else(|| "unknown".to_string());
    let version = sw_info.version.unwrap_or_else(|| "unknown".to_string());

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
    let serial_number = chassis::parse_serial_number(
        &chassis_items
            .first()
            .ok_or_else(|| RustEzError::Facts("empty chassis-inventory response".to_string()))?
            .1,
    )
    .unwrap_or_else(|| "unknown".to_string());

    // 3. Route engine information
    let re_xml = rpc_with_timeout(client, "<get-route-engine-information/>", timeout).await?;
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
        is_cluster,
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
            "facts RPC timed out after {timeout:?}"
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
                let name = local.as_ref();

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
                            item_content.push_str(attr.key.as_ref());
                            item_content.push_str("=\"");
                            // attr.value is the raw (already entity-escaped)
                            // wire value, so only the double-quote delimiter
                            // needs escaping — never re-escape `&`.
                            item_content.push_str(&attr.value.replace('"', "&quot;"));
                            item_content.push('"');
                        }
                        item_content.push('>');
                    }
                }
            }
            Ok(Event::Text(ref text)) => {
                // Since quick-xml 0.38, Text events never contain entity refs
                // (those arrive as GeneralRef). Since 0.42 the payload is
                // already `str`, so there is no decode step left at all.
                let value = &**text;
                if in_re_name {
                    current_re_name
                        .get_or_insert_with(String::new)
                        .push_str(value);
                } else if capturing {
                    item_content.push_str(value);
                }
            }
            Ok(Event::GeneralRef(ref entity)) if in_re_name => {
                // re-name is a leaf value: resolve the entity to its text.
                if let Some(resolved) = xml_entity::resolve_entity_ref(entity) {
                    current_re_name
                        .get_or_insert_with(String::new)
                        .push_str(&resolved);
                }
            }
            Ok(Event::GeneralRef(ref entity)) if capturing => {
                // item_content is reconstructed XML re-parsed downstream: keep the
                // reference escaped verbatim so the fragment stays well-formed.
                item_content.push_str(&xml_entity::raw_entity_ref(entity));
            }
            Ok(Event::Empty(ref tag)) if capturing => {
                let local = tag.local_name();
                let name = local.as_ref();
                item_content.push('<');
                item_content.push_str(name);
                for attr in tag.attributes().flatten() {
                    item_content.push(' ');
                    item_content.push_str(attr.key.as_ref());
                    item_content.push_str("=\"");
                    item_content.push_str(&attr.value);
                    item_content.push('"');
                }
                item_content.push_str("/>");
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = local.as_ref();

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
            Err(err) => {
                tracing::warn!("XML parse error in unwrap_multi_re: {err}");
                break;
            }
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
    fn test_unwrap_multi_re_preserves_entities() {
        // The reconstructed per-RE content is re-parsed downstream, so entity
        // refs must be kept verbatim (`&amp;`) to stay well-formed; re-name is
        // a leaf value, so its entity resolves. Regression for quick-xml 0.38+
        // GeneralRef handling.
        let xml = r#"<multi-routing-engine-results>
  <multi-routing-engine-item>
    <re-name>node&amp;0</re-name>
    <software-information>
      <host-name>a&amp;b&lt;c</host-name>
    </software-information>
  </multi-routing-engine-item>
</multi-routing-engine-results>"#;

        let items = unwrap_multi_re(xml);
        assert_eq!(items.len(), 1);
        // re-name value is resolved.
        assert_eq!(items[0].0.as_deref(), Some("node&0"));
        // Inner XML keeps the entity escaped so it re-parses correctly.
        assert!(items[0].1.contains("a&amp;b&lt;c"));
        let info = software::parse_software_info(&items[0].1);
        assert_eq!(info.hostname.as_deref(), Some("a&b<c"));
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

    #[test]
    fn test_cluster_detected_from_multi_re_items() {
        let xml = r#"<multi-routing-engine-results>
  <multi-routing-engine-item>
    <re-name>node0</re-name>
    <software-information><host-name>node0</host-name></software-information>
  </multi-routing-engine-item>
  <multi-routing-engine-item>
    <re-name>node1</re-name>
    <software-information><host-name>node1</host-name></software-information>
  </multi-routing-engine-item>
</multi-routing-engine-results>"#;

        let items = unwrap_multi_re(xml);
        let is_cluster = items.len() > 1;
        assert!(is_cluster);
    }

    #[test]
    fn test_single_re_not_cluster() {
        let xml = r#"<software-information>
  <host-name>vsrx1</host-name>
</software-information>"#;

        let items = unwrap_multi_re(xml);
        let is_cluster = items.len() > 1;
        assert!(!is_cluster);
    }

    #[test]
    fn facts_serializes_to_json_with_snake_case_fields() {
        let facts = Facts {
            hostname: "lab-r1.example.net".to_string(),
            model: "vSRX".to_string(),
            version: "23.4R1.10".to_string(),
            serial_number: "VM5A1234".to_string(),
            personality: Personality::Vsrx,
            route_engines: vec![RouteEngine {
                slot: Some(0),
                status: "OK".to_string(),
                model: Some("RE-VSRX".to_string()),
                mastership_state: Some("master".to_string()),
                uptime: Some("3 days".to_string()),
                memory_total: Some("4096 MB".to_string()),
            }],
            master_re: Some(0),
            domain: Some("example.net".to_string()),
            fqdn: Some("lab-r1.example.net".to_string()),
            is_cluster: false,
        };

        let value = serde_json::to_value(&facts).unwrap();

        // Top-level snake_case fields are preserved (no rename_all on Facts).
        assert_eq!(value["hostname"], "lab-r1.example.net");
        assert_eq!(value["serial_number"], "VM5A1234");
        assert_eq!(value["is_cluster"], false);
        assert_eq!(value["master_re"], 0);

        // Personality serializes as a lowercase string for known variants.
        assert_eq!(value["personality"], "vsrx");

        // Nested RouteEngine serializes with snake_case fields.
        let re = &value["route_engines"][0];
        assert_eq!(re["slot"], 0);
        assert_eq!(re["status"], "OK");
        assert_eq!(re["mastership_state"], "master");
        assert_eq!(re["memory_total"], "4096 MB");
    }

    #[test]
    fn personality_unknown_serializes_with_model_payload() {
        let p = Personality::Unknown("FutureRouter9000".to_string());
        let value = serde_json::to_value(&p).unwrap();
        // Unknown is the only variant carrying data — serializes as a tagged
        // object: {"unknown": "FutureRouter9000"}.
        assert_eq!(value["unknown"], "FutureRouter9000");
    }
}
