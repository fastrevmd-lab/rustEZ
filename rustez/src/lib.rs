//! # rustEZ
//!
//! A Rust replacement for Juniper PyEZ — async-first Junos device automation
//! built on [rustnetconf](https://github.com/fastrevmd-lab/rustnetconf).
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use rustez::Device;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut dev = Device::connect("10.0.0.1")
//!         .username("admin")
//!         .password("secret")
//!         .open()
//!         .await?;
//!
//!     let facts = dev.facts().await?;
//!     println!("{} running Junos {}", facts.hostname, facts.version);
//!
//!     dev.close().await?;
//!     Ok(())
//! }
//! ```

pub mod config;
pub mod device;
pub mod error;
pub mod facts;
pub mod rpc;

pub use config::{ConfigManager, ConfigPayload};
pub use device::{Device, DeviceBuilder};
pub use error::RustEzError;
pub use facts::{Facts, Personality, RouteEngine};
pub use rpc::RpcExecutor;

// Re-export rustnetconf types that users commonly need
pub use rustnetconf::Datastore;
