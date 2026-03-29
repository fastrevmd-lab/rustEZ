//! Python bindings for rustEZ via PyO3.
//!
//! Exposes a blocking `PyDevice` that wraps async rustez operations
//! using a per-device tokio runtime. Returns XML strings to Python;
//! the pure-Python layer parses them into lxml Elements.

use std::sync::Mutex;
use std::time::Duration;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use rustez::config::ConfigPayload;
use rustez::Device;

/// Convert a RustEzError to a Python RuntimeError string.
fn to_py_err(err: rustez::RustEzError) -> PyErr {
    PyRuntimeError::new_err(format!("{err}"))
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
    password: String,
    timeout: u64,
}

#[pymethods]
impl PyDevice {
    /// Create a new PyDevice (does NOT connect yet — call .open()).
    #[new]
    #[pyo3(signature = (host, username, password, port=830, timeout=30))]
    fn new(host: String, username: String, password: String, port: u16, timeout: u64) -> PyResult<Self> {
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
            password,
            timeout,
        })
    }

    /// Open the NETCONF connection and optionally gather facts.
    ///
    /// When `gather_facts` is False, the session connects without sending
    /// facts RPCs — useful for clustered SRX where a peer node is unreachable.
    #[pyo3(signature = (gather_facts=true))]
    fn open(&self, gather_facts: bool) -> PyResult<()> {
        let dev = self.runtime.block_on(async {
            let mut builder = Device::connect(&self.host)
                .port(self.port)
                .username(&self.username)
                .password(&self.password)
                .rpc_timeout(Duration::from_secs(self.timeout));

            if !gather_facts {
                builder = builder.no_facts();
            }

            builder.open().await
        }).map_err(to_py_err)?;

        let mut guard = self.device.lock().unwrap();
        *guard = Some(dev);
        Ok(())
    }

    /// Close the NETCONF connection.
    fn close(&self) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        if let Some(ref mut dev) = *guard {
            self.runtime.block_on(dev.close()).map_err(to_py_err)?;
        }
        *guard = None;
        Ok(())
    }

    /// Return facts as a Python dict.
    fn facts(&self) -> PyResult<Vec<(String, String)>> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let facts = self.runtime.block_on(dev.facts()).map_err(to_py_err)?;
        Ok(vec![
            ("hostname".to_string(), facts.hostname.clone()),
            ("model".to_string(), facts.model.clone()),
            ("version".to_string(), facts.version.clone()),
            ("serialnumber".to_string(), facts.serial_number.clone()),
            ("personality".to_string(), format!("{}", facts.personality)),
        ])
    }

    /// Execute a CLI command. Returns text output.
    fn cli(&self, command: &str) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        self.runtime.block_on(dev.cli(command)).map_err(to_py_err)
    }

    /// Execute a named RPC. Returns raw XML string.
    ///
    /// `rpc_name`: underscore-separated (e.g. "get_interface_information")
    /// `args`: list of (key, value) tuples
    fn rpc_call(&self, rpc_name: &str, args: Vec<(String, String)>) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut rpc = dev.rpc().map_err(to_py_err)?;

        let arg_refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.runtime.block_on(rpc.call(rpc_name, &arg_refs)).map_err(to_py_err)
    }

    /// Execute a CLI command via RPC, returning raw XML string.
    fn rpc_cli(&self, command: &str, format: &str) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut rpc = dev.rpc().map_err(to_py_err)?;
        self.runtime.block_on(rpc.cli(command, format)).map_err(to_py_err)
    }

    /// Send raw XML RPC. Returns raw XML string.
    fn rpc_xml(&self, xml: &str) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut rpc = dev.rpc().map_err(to_py_err)?;
        self.runtime.block_on(rpc.call_xml(xml)).map_err(to_py_err)
    }

    /// Lock the candidate config.
    fn config_lock(&self) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.lock()).map_err(to_py_err)
    }

    /// Unlock the candidate config.
    fn config_unlock(&self) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.unlock()).map_err(to_py_err)
    }

    /// Load config. format: "set", "text", or "xml".
    fn config_load(&self, content: &str, format: &str) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;

        let payload = match format {
            "set" => ConfigPayload::Set(content.to_string()),
            "text" => ConfigPayload::Text(content.to_string()),
            "xml" => ConfigPayload::Xml(content.to_string()),
            _ => return Err(PyRuntimeError::new_err(format!("unknown format: {format}"))),
        };

        self.runtime.block_on(cfg.load(payload)).map_err(to_py_err)
    }

    /// Get candidate diff. Returns diff string or empty string.
    fn config_diff(&self) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        let diff = self.runtime.block_on(cfg.diff()).map_err(to_py_err)?;
        Ok(diff.unwrap_or_default())
    }

    /// Commit candidate config.
    fn config_commit(&self) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.commit()).map_err(to_py_err)
    }

    /// Commit confirmed with rollback timer in seconds.
    fn config_commit_confirmed(&self, seconds: u32) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.commit_confirmed(seconds)).map_err(to_py_err)
    }

    /// Rollback to configuration N (0 = running).
    fn config_rollback(&self, id: u32) -> PyResult<String> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.rollback(id)).map_err(to_py_err)
    }

    /// Validate candidate config without committing.
    fn config_commit_check(&self) -> PyResult<()> {
        let mut guard = self.device.lock().unwrap();
        let dev = guard.as_mut().ok_or_else(|| PyRuntimeError::new_err("not connected"))?;
        let mut cfg = dev.config().map_err(to_py_err)?;
        self.runtime.block_on(cfg.commit_check()).map_err(to_py_err)
    }
}

/// The native extension module.
#[pymodule]
fn _rustez_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDevice>()?;
    Ok(())
}
