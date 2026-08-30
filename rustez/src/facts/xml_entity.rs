//! Helpers for quick-xml 0.38+ entity-reference events.
//!
//! Since quick-xml 0.38, entity references (`&amp;`, `&#38;`, …) no longer
//! arrive inside `Event::Text` — they stream as separate `Event::GeneralRef`
//! events. Fact-parser reader loops must resolve these and stitch them back
//! into the surrounding value, otherwise any Junos fact containing an entity
//! (descriptions, URLs, config text with `&`/`<`/`>`) is silently truncated.

use quick_xml::events::BytesRef;

/// Resolve a general entity reference to its decoded text value.
///
/// Handles numeric character references (`&#38;`, `&#x26;`) and the five
/// predefined XML entities (`amp`, `lt`, `gt`, `apos`, `quot`). Returns
/// `None` for unknown user-defined entities, which callers skip (matching
/// the old `unescape()` behavior of not inventing content).
pub(crate) fn resolve_entity_ref(entity: &BytesRef<'_>) -> Option<String> {
    if let Ok(Some(ch)) = entity.resolve_char_ref() {
        return Some(ch.to_string());
    }
    quick_xml::escape::resolve_predefined_entity(entity).map(|s| s.to_string())
}

/// Reconstruct the raw wire form (`&name;`) of an entity reference.
///
/// Used when rebuilding inner XML verbatim (e.g. per-RE content that is
/// re-parsed downstream): the reference is kept escaped so the reconstructed
/// fragment stays well-formed and round-trips exactly, including user-defined
/// entities we cannot resolve.
pub(crate) fn raw_entity_ref(entity: &BytesRef<'_>) -> String {
    format!("&{};", &**entity)
}
