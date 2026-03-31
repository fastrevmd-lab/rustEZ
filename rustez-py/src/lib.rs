//! Python bindings for rustEZ via PyO3.
//!
//! Exposes a blocking `PyDevice` that wraps async rustez operations
//! using a per-device tokio runtime. Returns XML strings to Python;
//! the pure-Python layer parses them into lxml Elements.
//!
//! All blocking network I/O releases the Python GIL so other threads
//! can run concurrently.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use rustez::config::ConfigPayload;
use rustez::{Device, OpenConfigurationMode};

/// Convert a RustEzError to a Python RuntimeError string.
fn to_py_err(err: rustez::RustEzError) -> PyErr {
    PyRuntimeError::new_err(format!("{err}"))
}

/// Lock a Mutex, converting poison errors to Python RuntimeError.
fn lock_mutex<T>(mutex: &Mutex<T>) -> PyResult<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| PyRuntimeError::new_err("internal lock poisoned"))
}

/// Native device handle. All methods are blocking (run async on internal tokio runtime).
///
/// The Python `rustez.Device` class wraps this and adds lxml parsing,
/// `__getattr__` RPC magic, and the familiar PyEZ-compatible API.
#[pyclass]
struct PyDevice {
    runtime: tokio::runtime::Runtime,
    device: Mutex<Option<Device>>,
    host: String,
    port: u16,
    username: String,
    password: Mutex<String>,
    timeout: u64,
    keepalive_interval: Option<u64>,
}

#[pymethods]
impl PyDevice {
    /// Create a new PyDevice (does NOT connect yet — call .open()).
    #[new]
    #[pyo3(signature = (host, username, password, port=830, timeout=30, keepalive_interval=None))]
    fn new(host: String, username: String, password: String, port: u16, timeout: u64, keepalive_interval: Option<u64>) -> PyResult<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PyRuntimeError::new_err(format!("tokio runtime: {e}")))?;

        Ok(PyDevice {
            runtime,
            device: Mutex::new(None),
            host,
            port,
            username,
            password: Mutex::new(password),
            timeout,
            keepalive_interval,
        })
    }

    /// Open the NETCONF connection and optionally gather facts.
    ///
    /// When `gather_facts` is False, the session connects without sending
    /// facts RPCs — useful for clustered SRX where a peer node is unreachable.
    #[pyo3(signature = (gather_facts=true))]
    fn open(&self, py: Python<'_>, gather_facts: bool) -> PyResult<()> {
        let password = {
            let guard = lock_mutex(&self.password)?;
            guard.clone()
        };

        let dev = py.allow_threads(|| {
            self.runtime.block_on(async {
                let mut builder = Device::connect(&self.host)
                    .port(self.port)
                    .username(&self.username)
                    .password(&password)
                    .rpc_timeout(Duration::from_secs(self.timeout));

                if !gather_facts {
                    builder = builder.no_facts();
                }
                if let Some(secs) = self.keepalive_interval {
                    builder = builder.keepalive_interval(Duration::from_secs(secs));
                }

                builder.open().await
            }).map_err(to_py_err)
        })?;

        // Clear password from memory after successful connection
        {
            let mut guard = lock_mutex(&self.password)?;
            guard.clear();
        }

        let mut guard = lock_mutex(&self.device)?;
        *guard = Some(dev);
        Ok(())
    }

    /// Check if the NETCONF session is alive (in-memory check, no RPC).
    fn session_alive(&self) -> PyResult<bool> {
        let guard = lock_mutex(&self.device)?;
        Ok(guard.as_ref().map_or(false, |dev| dev.session_alive()))
    }

    /// Reconnect to the device using the original connection parameters.
    fn reconnect(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            self.runtime.block_on(dev.reconnect()).map_err(to_py_err)
        })
    }

    /// Close the NETCONF connection.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            if let Some(ref mut dev) = *guard {
                self.runtime.block_on(dev.close()).map_err(to_py_err)?;
            }
            *guard = None;
            Ok(())
        })
    }

    /// Whether the device is part of a chassis cluster.
    fn is_cluster(&self) -> PyResult<bool> {
        let guard = lock_mutex(&self.device)?;
        Ok(guard.as_ref().map_or(false, |dev| dev.is_cluster()))
    }

    /// Return facts as a Python dict.
    fn facts(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let facts = self.runtime.block_on(dev.facts()).map_err(to_py_err)?;
            Ok(vec![
                ("hostname".to_string(), facts.hostname.clone()),
                ("model".to_string(), facts.model.clone()),
                ("version".to_string(), facts.version.clone()),
                ("serialnumber".to_string(), facts.serial_number.clone()),
                ("personality".to_string(), format!("{}", facts.personality)),
                ("is_cluster".to_string(), facts.is_cluster.to_string()),
            ])
        })
    }

    /// Execute a CLI command. Returns text output.
    fn cli(&self, py: Python<'_>, command: &str) -> PyResult<String> {
        let command = command.to_string();
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            self.runtime.block_on(dev.cli(&command)).map_err(to_py_err)
        })
    }

    /// Execute a named RPC. Returns raw XML string.
    ///
    /// `rpc_name`: underscore-separated (e.g. "get_interface_information")
    /// `args`: list of (key, value) tuples
    fn rpc_call(&self, py: Python<'_>, rpc_name: &str, args: Vec<(String, String)>) -> PyResult<String> {
        let rpc_name = rpc_name.to_string();
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut rpc = dev.rpc().map_err(to_py_err)?;

            let arg_refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            self.runtime.block_on(rpc.call(&rpc_name, &arg_refs)).map_err(to_py_err)
        })
    }

    /// Execute a CLI command via RPC, returning raw XML string.
    fn rpc_cli(&self, py: Python<'_>, command: &str, format: &str) -> PyResult<String> {
        let command = command.to_string();
        let format = format.to_string();
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut rpc = dev.rpc().map_err(to_py_err)?;
            self.runtime.block_on(rpc.cli(&command, &format)).map_err(to_py_err)
        })
    }

    /// Send raw XML RPC. Returns raw XML string.
    fn rpc_xml(&self, py: Python<'_>, xml: &str) -> PyResult<String> {
        let xml = xml.to_string();
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut rpc = dev.rpc().map_err(to_py_err)?;
            self.runtime.block_on(rpc.call_xml(&xml)).map_err(to_py_err)
        })
    }

    /// Send raw XML RPC and return any warnings alongside the response.
    ///
    /// Returns `(response_xml, [(severity, message), ...])`.
    fn rpc_xml_with_warnings(
        &self,
        py: Python<'_>,
        xml: &str,
    ) -> PyResult<(String, Vec<(String, String)>)> {
        let xml = xml.to_string();
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut rpc = dev.rpc().map_err(to_py_err)?;
            let (data, warnings) = self
                .runtime
                .block_on(rpc.call_xml_with_warnings(&xml))
                .map_err(to_py_err)?;
            let warning_tuples: Vec<(String, String)> = warnings
                .iter()
                .map(|w| {
                    let severity = w
                        .severity
                        .as_ref()
                        .map(|s| format!("{s:?}").to_lowercase())
                        .unwrap_or_else(|| "unknown".to_string());
                    (severity, w.message.clone())
                })
                .collect();
            Ok((data, warning_tuples))
        })
    }

    /// Open a private or exclusive configuration database (Junos clusters).
    ///
    /// `mode`: `"private"` or `"exclusive"`.
    #[pyo3(signature = (mode="private"))]
    fn config_open_configuration(&self, py: Python<'_>, mode: &str) -> PyResult<()> {
        let open_mode = match mode {
            "private" => OpenConfigurationMode::Private,
            "exclusive" => OpenConfigurationMode::Exclusive,
            _ => return Err(PyRuntimeError::new_err(format!("unknown mode: {mode}, use 'private' or 'exclusive'"))),
        };
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            self.runtime
                .block_on(dev.open_configuration(open_mode))
                .map_err(to_py_err)
        })
    }

    /// Close a previously opened configuration database.
    fn config_close_configuration(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            self.runtime
                .block_on(dev.close_configuration())
                .map_err(to_py_err)
        })
    }

    /// Lock the candidate config.
    fn config_lock(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.lock()).map_err(to_py_err)
        })
    }

    /// Unlock the candidate config.
    fn config_unlock(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.unlock()).map_err(to_py_err)
        })
    }

    /// Load config. format: "set", "text", or "xml".
    fn config_load(&self, py: Python<'_>, content: &str, format: &str) -> PyResult<String> {
        let payload = match format {
            "set" => ConfigPayload::Set(content.to_string()),
            "text" => ConfigPayload::Text(content.to_string()),
            "xml" => ConfigPayload::Xml(content.to_string()),
            _ => return Err(PyRuntimeError::new_err(format!("unknown format: {format}"))),
        };

        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.load(payload)).map_err(to_py_err)
        })
    }

    /// Get candidate diff. Returns diff string or empty string.
    fn config_diff(&self, py: Python<'_>) -> PyResult<String> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            let diff = self.runtime.block_on(cfg.diff()).map_err(to_py_err)?;
            Ok(diff.unwrap_or_default())
        })
    }

    /// Commit candidate config.
    fn config_commit(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.commit()).map_err(to_py_err)
        })
    }

    /// Commit confirmed with rollback timer in seconds.
    fn config_commit_confirmed(&self, py: Python<'_>, seconds: u32) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.commit_confirmed(seconds)).map_err(to_py_err)
        })
    }

    /// Rollback to configuration N (0 = running).
    fn config_rollback(&self, py: Python<'_>, id: u32) -> PyResult<String> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.rollback(id)).map_err(to_py_err)
        })
    }

    /// Validate candidate config without committing.
    fn config_commit_check(&self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let mut guard = lock_mutex(&self.device)?;
            let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
            let mut cfg = dev.config().map_err(to_py_err)?;
            self.runtime.block_on(cfg.commit_check()).map_err(to_py_err)
        })
    }
}

/// The native extension module.
#[pymodule]
fn _rustez_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDevice>()?;
    Ok(())
}
