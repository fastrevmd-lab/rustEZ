//! Junos device personality detection from model strings.

use std::fmt;

/// The personality (platform family) of a Junos device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Personality {
    Mx,
    Vmx,
    Srx,
    Vsrx,
    Ex,
    Qfx,
    Ptx,
    Acx,
    Nfx,
    M,
    T,
    Olive,
    Jdm,
    Unknown(String),
}

impl fmt::Display for Personality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Personality::Unknown(model) => write!(f, "Unknown({model})"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Detect the device personality from a model string.
///
/// Uses case-insensitive prefix/substring matching against known Junos
/// platform families. Order matters — more specific matches (e.g., "vmx"
/// before "mx", "vsrx" before "srx") are checked first.
pub fn detect_personality(model: &str) -> Personality {
    let lower = model.to_lowercase();

    if lower.contains("vmx") || lower.contains("re-vmx") {
        return Personality::Vmx;
    }
    if lower.starts_with("mx") {
        return Personality::Mx;
    }
    if lower.contains("vsrx") || lower.contains("firefly") {
        return Personality::Vsrx;
    }
    if lower.starts_with("srx") {
        return Personality::Srx;
    }
    if lower.starts_with("ex") {
        return Personality::Ex;
    }
    if lower.starts_with("qfx") {
        return Personality::Qfx;
    }
    if lower.starts_with("ptx") {
        return Personality::Ptx;
    }
    if lower.starts_with("acx") {
        return Personality::Acx;
    }
    if lower.starts_with("nfx") {
        return Personality::Nfx;
    }
    if lower.contains("olive") {
        return Personality::Olive;
    }
    if lower.starts_with("jdm") {
        return Personality::Jdm;
    }
    if lower.starts_with("m") && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
        return Personality::M;
    }
    if lower.starts_with("t") && lower.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
        return Personality::T;
    }

    Personality::Unknown(model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_models() {
        assert_eq!(detect_personality("vSRX"), Personality::Vsrx);
        assert_eq!(detect_personality("VSRX3"), Personality::Vsrx);
        assert_eq!(detect_personality("MX480"), Personality::Mx);
        assert_eq!(detect_personality("SRX340"), Personality::Srx);
        assert_eq!(detect_personality("EX4300"), Personality::Ex);
        assert_eq!(detect_personality("QFX5100"), Personality::Qfx);
        assert_eq!(detect_personality("PTX10008"), Personality::Ptx);
        assert_eq!(detect_personality("ACX5048"), Personality::Acx);
        assert_eq!(detect_personality("NFX250"), Personality::Nfx);
        assert_eq!(detect_personality("RE-VMX"), Personality::Vmx);
        assert_eq!(detect_personality("VMX"), Personality::Vmx);
        assert_eq!(detect_personality("olive"), Personality::Olive);
        assert_eq!(detect_personality("Firefly-Perimeter"), Personality::Vsrx);
        assert_eq!(detect_personality("M320"), Personality::M);
        assert_eq!(detect_personality("T640"), Personality::T);
    }

    #[test]
    fn test_unknown_model() {
        let result = detect_personality("SomeNewPlatform");
        assert_eq!(result, Personality::Unknown("SomeNewPlatform".to_string()));
    }
}
