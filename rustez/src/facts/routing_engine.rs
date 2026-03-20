//! Parser for `<route-engine-information>` RPC responses.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Information about a single routing engine.
#[derive(Debug, Clone, Default)]
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
                }
            }
            Ok(Event::Text(ref text)) if in_route_engine => {
                let value = text.unescape().unwrap_or_default().trim().to_string();
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
                        _ => {}
                    }
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
                    current_element.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    engines
}

/// Determine the master RE index from a list of route engines.
pub(crate) fn find_master_re(engines: &[RouteEngine]) -> Option<usize> {
    engines.iter().position(|re| {
        re.mastership_state
            .as_deref()
            .map(|s| s.to_lowercase().contains("master"))
            .unwrap_or(false)
    })
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
}
