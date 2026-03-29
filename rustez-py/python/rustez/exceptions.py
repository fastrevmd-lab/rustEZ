"""rustEZ exception types — drop-in replacements for jnpr.junos.exception.

Exception hierarchy::

    RustEzError
    ├── ConnectError
    │   ├── ConnectAuthError
    │   └── ConnectTimeoutError
    ├── TransportError
    │   ├── ChannelClosedError
    │   └── SessionExpiredError
    ├── RpcError
    │   ├── RpcTimeoutError
    │   └── MessageIdMismatchError
    └── ConfigLoadError
"""


class RustEzError(Exception):
    """Base exception for all rustEZ errors."""


class ConnectError(RustEzError):
    """Connection to device failed."""


class ConnectAuthError(ConnectError):
    """Authentication failed during connection."""


class ConnectTimeoutError(ConnectError):
    """Connection timed out."""


class TransportError(RustEzError):
    """Transport-layer error (SSH channel, network interruption)."""


class ChannelClosedError(TransportError):
    """SSH channel closed by remote (device reboot, SSH timeout, network drop)."""


class SessionExpiredError(TransportError):
    """Session expired — keepalive probe detected dead connection."""


class RpcError(RustEzError):
    """RPC execution failed on the device."""


class RpcTimeoutError(RpcError):
    """RPC response not received within deadline."""


class MessageIdMismatchError(RpcError):
    """Response message-id does not match request."""


class ConfigLoadError(RustEzError):
    """Configuration load failed (syntax error, invalid statement, etc.)."""


def classify_error(exc: Exception) -> RustEzError:
    """Classify a RuntimeError from the native module into a typed exception.

    Maps rustnetconf error string patterns to typed Python exceptions.

    Args:
        exc: The original RuntimeError from _rustez_native.

    Returns:
        A typed rustEZ exception.
    """
    msg = str(exc)
    msg_lower = msg.lower()

    # rustnetconf typed errors — match specific patterns first
    if "channel closed:" in msg_lower:
        return ChannelClosedError(msg)
    if "message-id mismatch:" in msg_lower:
        return MessageIdMismatchError(msg)
    if "session expired:" in msg_lower:
        return SessionExpiredError(msg)

    # RPC timeout (rustnetconf "RPC timeout after ..." or rustez "timeout:")
    if "rpc timeout" in msg_lower or msg_lower.startswith("timeout:"):
        return RpcTimeoutError(msg)

    # Auth errors
    if "auth" in msg_lower or "permission denied" in msg_lower or "authentication" in msg_lower:
        return ConnectAuthError(msg)

    # Connection timeout (non-RPC)
    if "timed out" in msg_lower or "timeout" in msg_lower:
        return ConnectTimeoutError(msg)

    # Transport errors
    if "channel error:" in msg_lower or "transport error:" in msg_lower:
        return TransportError(msg)

    # General connection errors
    if "connect" in msg_lower or "connection" in msg_lower:
        return ConnectError(msg)

    # RPC server errors
    if "server error:" in msg_lower or "rpc error:" in msg_lower:
        return RpcError(msg)

    # Config errors
    if "config" in msg_lower or "load" in msg_lower:
        return ConfigLoadError(msg)

    return RustEzError(msg)
