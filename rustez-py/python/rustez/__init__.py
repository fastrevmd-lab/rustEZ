"""rustEZ — Python bindings for Rust-based Junos device automation.

Drop-in replacement for jnpr.junos (PyEZ). Uses rustnetconf under the hood
for NETCONF transport, exposed via PyO3 native bindings.

Usage::

    from rustez import Device, Config
    from rustez.exceptions import ConnectError, RpcError

    dev = Device(host="10.0.0.1", user="admin", passwd="secret", port=22)
    dev.open()
    print(dev.facts.get("hostname"))
    dev.rpc.get_interface_information(terse=True)
    dev.close()
"""

from rustez._rustez_native import PyDevice as _PyDevice
from rustez.exceptions import classify_error, ConfigLoadError

from lxml import etree


class _FactsDict(dict):
    """Dict subclass for device facts — mirrors PyEZ dev.facts interface."""
    pass


class _RpcProxy:
    """Proxy that translates attribute access to NETCONF RPCs.

    Supports two calling conventions:
    - Named RPCs: dev.rpc.get_interface_information(terse=True)
    - CLI: dev.rpc.cli("show version", format="xml")
    - get_config: dev.rpc.get_config(filter_xml="<...>", options={"format": "set"})

    Bool kwargs become empty XML elements. String kwargs become text elements.
    Returns lxml.etree.Element.
    """

    def __init__(self, native: _PyDevice) -> None:
        """Initialize with native device handle.

        Args:
            native: The underlying _rustez_native.PyDevice instance.
        """
        self._native = native

    def __getattr__(self, name: str):
        """Build and execute an RPC from attribute access.

        Args:
            name: The RPC name with underscores (converted to hyphens).

        Returns:
            A callable that executes the RPC and returns an lxml Element.
        """
        if name.startswith("_"):
            raise AttributeError(name)

        def _rpc_call(*args, **kwargs):
            """Execute the named RPC with positional and keyword arguments.

            For rpc.cli(), the first positional arg is the command string.
            For rpc.get_config(), keyword args are used.
            For named RPCs, keyword args become XML child elements.
            """
            if name == "cli" and args:
                kwargs["command"] = args[0]
            return self._dispatch(name, kwargs)

        return _rpc_call

    def _dispatch(self, name: str, kwargs: dict):
        """Dispatch an RPC call to the native module.

        Args:
            name: RPC name (underscored).
            kwargs: Keyword arguments for the RPC.

        Returns:
            lxml.etree.Element parsed from the XML response.
        """
        try:
            if name == "cli":
                return self._do_cli(kwargs)
            if name == "get_config":
                return self._do_get_config(kwargs)
            return self._do_named_rpc(name, kwargs)
        except RuntimeError as exc:
            raise classify_error(exc) from exc

    def _do_cli(self, kwargs: dict):
        """Execute a CLI command via RPC.

        Args:
            kwargs: Must contain positional command as first arg.
                format: Output format (default "text").

        Returns:
            lxml Element with CLI output.
        """
        command = kwargs.pop("command", None)
        if command is None:
            # cli() is called as rpc.cli("show version", format="xml")
            # but also could be passed as keyword
            raise ValueError("cli() requires a command argument")
        fmt = kwargs.pop("format", "text")
        xml_str = self._native.rpc_cli(command, fmt)
        return _parse_xml(xml_str)

    def _do_get_config(self, kwargs: dict):
        """Execute get-config RPC.

        Args:
            kwargs: filter_xml (optional), options (optional dict with 'format' key).

        Returns:
            lxml Element with configuration data.
        """
        filter_xml = kwargs.get("filter_xml")
        options = kwargs.get("options", {})
        fmt = options.get("format", "xml")

        # Build the get-config RPC XML
        rpc_xml = f'<get-configuration format="{fmt}"'
        if filter_xml:
            rpc_xml += f">{filter_xml}</get-configuration>"
        else:
            rpc_xml += "/>"

        xml_str = self._native.rpc_xml(rpc_xml)
        return _parse_xml(xml_str)

    def _do_named_rpc(self, name: str, kwargs: dict):
        """Execute a named RPC with keyword arguments.

        Bool True values become empty elements. String values become text elements.

        Args:
            name: RPC name (underscored, will be hyphenated by native layer).
            kwargs: Key-value arguments.

        Returns:
            lxml Element parsed from response XML.
        """
        args = []
        for key, value in kwargs.items():
            if isinstance(value, bool) and value:
                args.append((key, ""))
            elif isinstance(value, str):
                args.append((key, value))
            elif value is not None:
                args.append((key, str(value)))
        xml_str = self._native.rpc_call(name, args)
        return _parse_xml(xml_str)


class Device:
    """Junos device connection — drop-in replacement for jnpr.junos.Device.

    Usage::

        dev = Device(host="10.0.0.1", user="admin", passwd="secret")
        dev.open()
        print(dev.facts["hostname"])
        dev.rpc.get_interface_information(terse=True)
        dev.close()
    """

    def __init__(
        self,
        host: str,
        user: str = "root",
        passwd: str = "",
        port: int = 830,
        timeout: int = 30,
        **kwargs,
    ) -> None:
        """Initialize a device connection (does not connect yet).

        Args:
            host: Device hostname or IP.
            user: SSH username.
            passwd: SSH password.
            port: NETCONF port (default 830).
            timeout: Per-RPC timeout in seconds.
            **kwargs: Ignored (for PyEZ compat).
        """
        self._native = _PyDevice(
            host=host,
            username=user,
            password=passwd,
            port=port,
            timeout=timeout,
        )
        self._facts: _FactsDict = _FactsDict()
        self._rpc = _RpcProxy(self._native)
        self._host = host
        self._port = port
        self._connected = False

    def open(self, gather_facts=True):
        """Open the NETCONF connection and optionally gather facts.

        Args:
            gather_facts: If False, skip facts RPCs on connect. Useful for
                clustered SRX where a peer node is unreachable.

        Returns:
            self (for chaining).

        Raises:
            ConnectError: On connection failure.
            ConnectAuthError: On authentication failure.
            ConnectTimeoutError: On timeout.
        """
        try:
            self._native.open(gather_facts=gather_facts)
        except RuntimeError as exc:
            raise classify_error(exc) from exc

        # Populate facts dict from native (skip if facts weren't gathered)
        if gather_facts:
            try:
                raw_facts = self._native.facts()
                self._facts = _FactsDict(raw_facts)
            except RuntimeError:
                self._facts = _FactsDict()
        else:
            self._facts = _FactsDict()

        self._connected = True
        return self

    def close(self) -> None:
        """Close the NETCONF connection."""
        try:
            self._native.close()
        except RuntimeError:
            pass
        self._connected = False

    @property
    def facts(self) -> _FactsDict:
        """Return device facts dict (hostname, model, version, etc.)."""
        return self._facts

    @property
    def rpc(self) -> _RpcProxy:
        """Return the RPC proxy for executing NETCONF RPCs."""
        return self._rpc

    @property
    def connected(self) -> bool:
        """Return True if the device is connected."""
        return self._connected

    def cli(self, command: str, warning: bool = True) -> str:
        """Execute a CLI command and return text output.

        Args:
            command: Junos CLI command string.
            warning: Ignored (PyEZ compat).

        Returns:
            Command output as a string.
        """
        try:
            return self._native.cli(command)
        except RuntimeError as exc:
            raise classify_error(exc) from exc


class Config:
    """Configuration utility — drop-in replacement for jnpr.junos.utils.config.Config.

    Usage::

        cu = Config(dev)
        cu.lock()
        cu.load("set system host-name test", format="set")
        print(cu.diff())
        cu.commit(comment="test change")
        cu.unlock()
    """

    def __init__(self, dev: Device) -> None:
        """Initialize with a connected Device.

        Args:
            dev: A connected rustez.Device instance.
        """
        self._native = dev._native

    def lock(self) -> None:
        """Lock the candidate configuration.

        Raises:
            RpcError: If lock fails (already locked, etc.).
        """
        try:
            self._native.config_lock()
        except RuntimeError as exc:
            raise classify_error(exc) from exc

    def unlock(self) -> None:
        """Unlock the candidate configuration."""
        try:
            self._native.config_unlock()
        except RuntimeError as exc:
            raise classify_error(exc) from exc

    def load(self, content: str, format: str = "xml", **kwargs) -> None:
        """Load configuration into the candidate datastore.

        Args:
            content: Configuration content string.
            format: Format — 'set', 'text', or 'xml'.
            **kwargs: Ignored (PyEZ compat: overwrite, merge, etc.).

        Raises:
            ConfigLoadError: If the load fails.
        """
        try:
            self._native.config_load(content, format)
        except RuntimeError as exc:
            typed = classify_error(exc)
            if not isinstance(typed, ConfigLoadError):
                typed = ConfigLoadError(str(exc))
            raise typed from exc

    def diff(self, rb_id: int = 0) -> str | None:
        """Show the candidate diff (uncommitted changes).

        Args:
            rb_id: Rollback ID to compare against (default 0 = running).

        Returns:
            Diff string, or None if no changes.
        """
        try:
            result = self._native.config_diff()
            return result if result else None
        except RuntimeError as exc:
            raise classify_error(exc) from exc

    def commit(self, comment: str = "", confirm: int = 0, **kwargs) -> None:
        """Commit the candidate configuration.

        Args:
            comment: Optional commit comment (note: rustEZ commit comment
                support requires the commit RPC to include <log> element —
                TODO: add to native layer).
            confirm: If > 0, use commit-confirmed with this many minutes.
            **kwargs: Ignored (PyEZ compat).

        Raises:
            RpcError: If commit fails.
        """
        try:
            if confirm > 0:
                self._native.config_commit_confirmed(confirm * 60)
            else:
                self._native.config_commit()
        except RuntimeError as exc:
            raise classify_error(exc) from exc

    def rollback(self, rb_id: int = 0) -> None:
        """Rollback candidate config.

        Args:
            rb_id: Rollback ID (default 0 = revert to running).
        """
        try:
            self._native.config_rollback(rb_id)
        except RuntimeError as exc:
            raise classify_error(exc) from exc


def _strip_namespaces(element):
    """Strip all XML namespaces from an element tree in-place.

    PyEZ returns namespace-free elements. rustnetconf preserves Junos
    namespaces (e.g. xmlns="http://xml.juniper.net/..."). Strip them
    so existing Outpost code using bare tag names (element.find("name"))
    continues to work.

    Args:
        element: lxml Element to strip namespaces from.

    Returns:
        The same element, modified in-place.
    """
    for el in element.iter():
        if isinstance(el.tag, str) and el.tag.startswith("{"):
            el.tag = el.tag.split("}", 1)[1]
        # Strip namespace-prefixed attributes (junos:style, etc.)
        attribs_to_remove = [k for k in el.attrib if k.startswith("{")]
        for attr_key in attribs_to_remove:
            local_name = attr_key.split("}", 1)[1]
            el.attrib[local_name] = el.attrib.pop(attr_key)
    return element


def _fix_orphaned_ns_prefixes(xml_str: str) -> str:
    """Add missing xmlns declarations for Junos namespace prefixes.

    rustnetconf strips the <rpc-reply> wrapper which carries xmlns:junos.
    The inner XML still has junos: prefixed attributes (e.g. junos:style)
    that become invalid without the declaration. Re-inject it.

    Args:
        xml_str: Raw XML that may have orphaned junos: prefixes.

    Returns:
        XML string with xmlns:junos declaration added if needed.
    """
    import re
    if "junos:" not in xml_str:
        return xml_str
    if "xmlns:junos=" in xml_str:
        return xml_str
    # Inject xmlns:junos on the root element
    return re.sub(
        r"^(<\s*[\w-]+)",
        r'\1 xmlns:junos="http://xml.juniper.net/junos"',
        xml_str,
        count=1,
    )


def _parse_xml(xml_str: str):
    """Parse an XML string into an lxml Element with namespaces stripped.

    Handles both full XML documents and fragments. Fixes orphaned Junos
    namespace prefixes and strips all namespaces so that Outpost code can
    use bare tag names (matching PyEZ behavior).

    Args:
        xml_str: Raw XML string from the native module.

    Returns:
        lxml.etree.Element with namespaces stripped.
    """
    xml_str = xml_str.strip()
    if not xml_str:
        return etree.Element("empty")

    xml_str = _fix_orphaned_ns_prefixes(xml_str)

    try:
        el = etree.fromstring(xml_str.encode("utf-8"))
        return _strip_namespaces(el)
    except etree.XMLSyntaxError:
        # Try wrapping in a root element for fragments
        try:
            wrapped = f"<rpc-reply>{xml_str}</rpc-reply>"
            el = etree.fromstring(wrapped.encode("utf-8"))
            return _strip_namespaces(el)
        except etree.XMLSyntaxError:
            # Last resort: return text in an element
            el = etree.Element("output")
            el.text = xml_str
            return el
