//! Parser for `<route-engine-information>` RPC responses.

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

/// Information about a single routing engine.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RouteEngine {
    /// RE slot number (e.g., 0, 1).
    pub slot: Option<u32>,
    /// Operational status (e.g., "OK").
    pub status: String,
    /// RE model string.
    pub model: Option<String>,
    /// Mastership state (e.g., "master", "backup").
    pub mastership_state: Option<String>,
    /// Uptime string.
    pub uptime: Option<String>,
    /// Total memory string.
    pub memory_total: Option<String>,
}

/// Parse `<route-engine-information>` XML into a list of route engines.
pub(crate) fn parse_route_engines(xml: &str) -> Vec<RouteEngine> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut engines = Vec::new();

    let mut current_engine: Option<RouteEngine> = None;
    let mut current_element = String::new();
    let mut in_route_engine = false;
    let mut text_buf = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref())
                    .unwrap_or("")
                    .to_string();

                if name == "route-engine" {
                    in_route_engine = true;
                    let mut engine = RouteEngine::default();
                    // Check for slot attribute
                    for attr in tag.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                        if key == "slot" || key.ends_with(":slot") {
                            if let Ok(slot) = String::from_utf8_lossy(&attr.value).parse::<u32>() {
                                engine.slot = Some(slot);
                            }
                        }
                    }
                    current_engine = Some(engine);
                } else if in_route_engine {
                    current_element = name;
                    // Reset per-element buffer so only this element's own text
                    // (not inter-element whitespace) is captured.
                    text_buf.clear();
                }
            }
            // Accumulate across Text/GeneralRef; since quick-xml 0.38 entity
            // refs arrive as separate GeneralRef events. Flush on closing tag.
            Ok(Event::Text(ref text)) if in_route_engine => {
                text_buf.push_str(&text.decode().unwrap_or_default());
            }
            Ok(Event::GeneralRef(ref entity)) if in_route_engine => {
                if let Some(resolved) = super::xml_entity::resolve_entity_ref(entity) {
                    text_buf.push_str(&resolved);
                }
            }
            Ok(Event::End(ref tag)) => {
                let local = tag.local_name();
                let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
                if name == "route-engine" {
                    in_route_engine = false;
                    if let Some(engine) = current_engine.take() {
                        engines.push(engine);
                    }
                } else if in_route_engine {
                    let value = std::mem::take(&mut text_buf).trim().to_string();
                    if let Some(ref mut engine) = current_engine {
                        match current_element.as_str() {
                            "slot" => {
                                if let Ok(slot) = value.parse::<u32>() {
                                    engine.slot = Some(slot);
                                }
                            }
                            "status" => engine.status = value,
                            "model" => engine.model = Some(value),
                            "mastership-state" => engine.mastership_state = Some(value),
                            "up-time" => engine.uptime = Some(value),
                            "memory-dram-size" | "memory-installed-size" => {
                                engine.memory_total = Some(value);
                            }
                            "memory-system-total" => {
                                // vSRX emits bare numbers; normalize to "N MB" for consistency
                                // with MX/RE-VMX format. Only append if no unit is already present.
                                let normalized = if value.chars().all(|c| c.is_ascii_digit()) {
                                    format!("{} MB", value)
                                } else {
                                    value
                                };
                                engine.memory_total = Some(normalized);
                            }
                            _ => {}
                        }
                    }
                    current_element.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                tracing::warn!("XML parse error in routing engine facts: {err}");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    engines
}

/// Determine the master RE index from a list of route engines.
///
/// Returns the index of the RE whose mastership-state contains "master".
///
/// Standalone platforms (e.g. vSRX) omit `<mastership-state>` entirely. When no
/// RE reports any mastership state and there is exactly one RE, it is treated as
/// the master. A lone RE that explicitly reports a non-master state is left
/// alone — the device's own answer wins over the standalone inference.
pub(crate) fn find_master_re(engines: &[RouteEngine]) -> Option<usize> {
    let master_idx = engines.iter().position(|re| {
        re.mastership_state
            .as_deref()
            .map(|state| state.to_lowercase().contains("master"))
            .unwrap_or(false)
    });

    if master_idx.is_some() {
        return master_idx;
    }

    // Only infer mastership when the device reported none at all; a single RE
    // that explicitly said "backup" must not be reported as master.
    let any_state_reported = engines.iter().any(|re| re.mastership_state.is_some());
    if engines.len() == 1 && !any_state_reported {
        return Some(0);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_re() {
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>0</slot>
    <status>OK</status>
    <mastership-state>master</mastership-state>
    <model>RE-VMX</model>
    <up-time>14 days, 3:22</up-time>
    <memory-dram-size>4096 MB</memory-dram-size>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].slot, Some(0));
        assert_eq!(engines[0].status, "OK");
        assert_eq!(engines[0].mastership_state.as_deref(), Some("master"));
        assert_eq!(engines[0].model.as_deref(), Some("RE-VMX"));
        assert_eq!(engines[0].uptime.as_deref(), Some("14 days, 3:22"));
        assert_eq!(engines[0].memory_total.as_deref(), Some("4096 MB"));

        assert_eq!(find_master_re(&engines), Some(0));
    }

    #[test]
    fn test_multi_re() {
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>0</slot>
    <status>OK</status>
    <mastership-state>master</mastership-state>
  </route-engine>
  <route-engine>
    <slot>1</slot>
    <status>OK</status>
    <mastership-state>backup</mastership-state>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 2);
        assert_eq!(engines[0].slot, Some(0));
        assert_eq!(engines[0].mastership_state.as_deref(), Some("master"));
        assert_eq!(engines[1].slot, Some(1));
        assert_eq!(engines[1].mastership_state.as_deref(), Some("backup"));

        assert_eq!(find_master_re(&engines), Some(0));
    }

    #[test]
    fn test_route_engine_field_with_entities() {
        // A field value containing entities must round-trip through the
        // Text/GeneralRef split introduced in quick-xml 0.38.
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>0</slot>
    <status>OK</status>
    <model>RE&amp;A&lt;B</model>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].model.as_deref(), Some("RE&A<B"));
    }

    #[test]
    fn test_vsrx_standalone() {
        // Real vSRX 24.4R1.9 output — bareword memory-system-total, no mastership-state, no slot element
        let xml = r#"<rpc-reply xmlns:junos="http://xml.juniper.net/junos/24.4R1.9/junos">
    <route-engine-information xmlns="http://xml.juniper.net/junos/24.4R0/junos-chassis">
        <route-engine>
            <status>Testing</status>
            <memory-system-total>16323</memory-system-total>
            <memory-system-total-used>10610</memory-system-total-used>
            <memory-system-total-util>65</memory-system-total-util>
            <memory-control-plane>4035</memory-control-plane>
            <memory-control-plane-used>1735</memory-control-plane-used>
            <memory-control-plane-util>43</memory-control-plane-util>
            <memory-data-plane>12288</memory-data-plane>
            <memory-data-plane-used>8847</memory-data-plane-used>
            <memory-data-plane-util>72</memory-data-plane-util>
            <cpu-user>2</cpu-user>
            <cpu-background>0</cpu-background>
            <cpu-system>8</cpu-system>
            <cpu-interrupt>0</cpu-interrupt>
            <cpu-idle>90</cpu-idle>
            <model>VSRX RE</model>
            <start-time junos:seconds="1784429468">2026-07-19 02:51:08 UTC</start-time>
            <up-time junos:seconds="45069">12 hours, 31 minutes, 9 seconds</up-time>
            <last-reboot-reason>Router rebooted after a normal shutdown.</last-reboot-reason>
            <load-average-one>9.62</load-average-one>
            <load-average-five>9.67</load-average-five>
            <load-average-fifteen>9.63</load-average-fifteen>
        </route-engine>
    </route-engine-information>
</rpc-reply>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].memory_total.as_deref(), Some("16323 MB"));
        assert_eq!(engines[0].mastership_state, None);
        assert_eq!(engines[0].slot, None);
        assert_eq!(engines[0].status, "Testing");
        assert_eq!(engines[0].model.as_deref(), Some("VSRX RE"));
        assert_eq!(
            engines[0].uptime.as_deref(),
            Some("12 hours, 31 minutes, 9 seconds")
        );

        // Single RE with no mastership state should be master
        assert_eq!(find_master_re(&engines), Some(0));
    }

    #[test]
    fn test_two_res_no_mastership() {
        // Two REs with no mastership-state should yield None
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>0</slot>
    <status>OK</status>
    <model>RE-VMX</model>
  </route-engine>
  <route-engine>
    <slot>1</slot>
    <status>OK</status>
    <model>RE-VMX</model>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 2);
        assert_eq!(engines[0].mastership_state, None);
        assert_eq!(engines[1].mastership_state, None);

        // Two REs with no mastership state should yield None (don't guess)
        assert_eq!(find_master_re(&engines), None);
    }

    #[test]
    fn test_memory_with_existing_unit() {
        // Value already carrying a unit must not be double-suffixed
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>0</slot>
    <status>OK</status>
    <memory-dram-size>4096 MB</memory-dram-size>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].memory_total.as_deref(), Some("4096 MB"));
    }

    #[test]
    fn test_memory_system_total_with_existing_unit() {
        // Exercises the normalization branch itself: if a platform ever emits
        // memory-system-total already carrying a unit, it must pass through
        // untouched rather than becoming "16323 MB MB".
        let xml = r#"<route-engine-information>
  <route-engine>
    <status>Testing</status>
    <memory-system-total>16323 MB</memory-system-total>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].memory_total.as_deref(), Some("16323 MB"));
    }

    #[test]
    fn test_single_re_explicit_backup_is_not_master() {
        // A lone RE that explicitly reports a non-master state must not be
        // promoted by the standalone inference — the device's answer wins.
        let xml = r#"<route-engine-information>
  <route-engine>
    <slot>1</slot>
    <status>OK</status>
    <mastership-state>backup</mastership-state>
  </route-engine>
</route-engine-information>"#;

        let engines = parse_route_engines(xml);
        assert_eq!(engines.len(), 1);
        assert_eq!(engines[0].mastership_state.as_deref(), Some("backup"));
        assert_eq!(find_master_re(&engines), None);
    }
}
