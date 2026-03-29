"""rustEZ exception types — drop-in replacements for jnpr.junos.exception."""


class RustEzError(Exception):
    """Base exception for all rustEZ errors."""


class ConnectError(RustEzError):
    """Connection to device failed."""


class ConnectAuthError(ConnectError):
    """Authentication failed during connection."""


class ConnectTimeoutError(ConnectError):
    """Connection timed out."""


class RpcError(RustEzError):
    """RPC execution failed on the device."""


class ConfigLoadError(RustEzError):
    """Configuration load failed (syntax error, invalid statement, etc.)."""


def classify_error(exc: Exception) -> RustEzError:
    """Classify a RuntimeError from the native module into a typed exception.

    Inspects the error message to determine the appropriate exception type.

    Args:
        exc: The original RuntimeError from _rustez_native.

    Returns:
        A typed rustEZ exception.
    """
    msg = str(exc).lower()

    if "auth" in msg or "permission denied" in msg or "authentication" in msg:
        return ConnectAuthError(str(exc))
    if "timed out" in msg or "timeout" in msg:
        return ConnectTimeoutError(str(exc))
    if "connect" in msg or "connection" in msg or "transport" in msg:
        return ConnectError(str(exc))
    if "rpc" in msg:
        return RpcError(str(exc))
    if "config" in msg or "load" in msg:
        return ConfigLoadError(str(exc))

    return RustEzError(str(exc))
