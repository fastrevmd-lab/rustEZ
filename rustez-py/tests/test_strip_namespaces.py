"""Unit tests for rustez._strip_namespaces.

These tests exercise the helper directly (no device required). They
guard against regression of #14 where stripped tags still serialized
with xmlns declarations, causing Junos action="replace" loads to merge
into a namespaced phantom subtree instead of replacing the candidate.
"""

from lxml import etree

from rustez import _strip_namespaces


def test_strips_clark_notation_tags():
    """Tags in Clark notation are rewritten to local names."""
    root = etree.fromstring(
        b'<configuration xmlns="http://yang.juniper.net/junos-es/conf/root">'
        b'<security xmlns="http://yang.juniper.net/junos-es/conf/security"/>'
        b"</configuration>"
    )
    _strip_namespaces(root)
    assert root.tag == "configuration"
    assert root[0].tag == "security"


def test_serialized_output_has_no_xmlns():
    """etree.tostring() must not emit xmlns declarations after stripping.

    This is the regression guard for #14. Before the fix, lxml's nsmap
    persisted even after tag rewrites, so tostring() re-emitted the
    original xmlns attributes.
    """
    root = etree.fromstring(
        b'<configuration xmlns="http://yang.juniper.net/junos-es/conf/root">'
        b'<security xmlns="http://yang.juniper.net/junos-es/conf/security">'
        b"<policies/>"
        b"</security>"
        b"</configuration>"
    )
    _strip_namespaces(root)
    serialized = etree.tostring(root).decode("utf-8")
    assert "xmlns" not in serialized, serialized


def test_strips_namespaced_attributes():
    """junos:-prefixed attributes are rewritten to their local name."""
    root = etree.fromstring(
        b'<configuration xmlns:junos="http://xml.juniper.net/junos" '
        b'junos:changed-seconds="1700000000"/>'
    )
    _strip_namespaces(root)
    assert "changed-seconds" in root.attrib
    assert root.attrib["changed-seconds"] == "1700000000"
    serialized = etree.tostring(root).decode("utf-8")
    assert "xmlns" not in serialized, serialized


def test_returns_same_element():
    """Function mutates in place and returns the same element."""
    root = etree.fromstring(b"<foo/>")
    assert _strip_namespaces(root) is root
