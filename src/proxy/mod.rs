//! Proxy contracts — the definition of a device *class*.
//!
//! A proxy is the fixed vocabulary for a kind of device: a `light` is a `light` whether it is
//! DALI, Zigbee, or a relay behind a contactor. Drivers bend to fit; nothing else in the
//! system speaks a vendor protocol.
//!
//! - [`schema`] — the shape of a `proxies/*.toml` file.
//! - [`registry`] — the contracts themselves, compiled in.
//! - [`resolved`] — narrowing a contract to what one driver actually implements.
//!
//! Resolution is the load-bearing step. A driver declares its capabilities; [`Proxy::resolve`]
//! returns the subset of the contract that is callable. Nothing downstream — UI, automation
//! editor, AI tool surface — ever sees a command the device cannot honour, so an unsupported
//! feature is an *absence* rather than a runtime error.

pub mod registry;
pub mod resolved;
pub mod schema;

pub use registry::ProxyRegistry;
pub use resolved::{CallError, Resolved};
pub use schema::{Capability, Param, Proxy, Signature, StateField, ValueType};
