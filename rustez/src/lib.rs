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
