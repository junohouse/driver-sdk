//! Write a Juno driver.
//!
//! A driver is one type implementing [`DriverModule`] and one line of glue:
//!
//! ```ignore
//! use juno_driver_sdk::*;
//!
//! struct MyLight;
//!
//! impl DriverModule for MyLight {
//!     fn on_command(&self, inst: &mut Instance, proxy: LocalId, cmd: &str, args: &Args)
//!         -> Vec<HostCall>
//!     {
//!         match cmd {
//!             "on"  => vec![HostCall::Http(HttpRequest::new("POST", "…"))],
//!             other => vec![HostCall::warn(format!("unhandled `{other}`"))],
//!         }
//!     }
//! }
//!
//! export_driver!(MyLight);
//! ```
//!
//! ```toml
//! # Cargo.toml
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! `cargo build --release` produces the driver file; `junod pack` wraps it and the manifest
//! into a `.junodrv` that a controller can install.
//!
//! # This crate depends on nothing of Juno's
//!
//! It is the *contract*, not an excerpt of the controller. It used to re-export the
//! controller's own types, which meant every driver compiled the whole controller — a private
//! repository — just to see three structs. Owning the types here inverts that: the controller
//! depends on this crate, drivers depend on this crate, and writing a driver needs no access
//! to anything of ours.
//!
//! Only `serde` and `serde_json` are needed, because the boundary is JSON either way.
//!
//! # What the macro does
//!
//! Exports three C entry points a controller looks for. Everything crosses as JSON, so your
//! driver never shares a Rust type with the controller and cannot be broken by a compiler
//! version mismatch. You do not call these; they exist so `unsafe` lives here rather than in
//! every driver.

pub mod host;

pub use host::{
    Args, Candidate, DeviceId, DriverModule, Field, HostCall, HttpRequest, Instance, PickRow,
    Request, Response, SetupStep, dispatch,
};
pub use serde_json::{Value, json};

/// Identifies one proxy, control, or connection *within a single driver's manifest*.
///
/// Local to the manifest, not global to the house: two drivers both numbering their first
/// proxy `1` is normal and expected. The controller pairs it with a device to get something
/// unique.
pub type LocalId = u32;

/// The ABI this SDK speaks. A controller refuses a driver built against a different one.
pub const ABI_VERSION: u32 = 1;

/// Turn a [`DriverModule`] into a loadable driver file.
#[macro_export]
macro_rules! export_driver {
    ($ty:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn juno_abi_version() -> u32 {
            $crate::ABI_VERSION
        }

        /// The controller hands us a JSON request and gets JSON back. The buffer we return is
        /// ours to free — see `juno_free`.
        ///
        /// # Safety
        /// `ptr`/`len` must describe a readable buffer; `out_len` must be writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn juno_call(
            ptr: *const u8,
            len: usize,
            out_len: *mut usize,
        ) -> *mut u8 {
            let request: $crate::Request = {
                let bytes = unsafe { ::std::slice::from_raw_parts(ptr, len) };
                match ::serde_json::from_slice(bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        return $crate::__respond(
                            $crate::Response {
                                calls: vec![$crate::HostCall::warn(format!(
                                    "driver could not read the request: {e}"
                                ))],
                                ..Default::default()
                            },
                            out_len,
                        );
                    }
                }
            };

            // A panicking driver must not unwind across the C boundary — that is undefined
            // behaviour. Catch it and report it as a warning instead.
            let response = ::std::panic::catch_unwind(|| {
                let module = <$ty as ::std::default::Default>::default();
                $crate::dispatch(&module, request)
            })
            .unwrap_or_else(|_| $crate::Response {
                calls: vec![$crate::HostCall::warn("driver panicked")],
                ..Default::default()
            });

            $crate::__respond(response, out_len)
        }

        /// Free a buffer `juno_call` returned. The driver allocated it, so the driver frees
        /// it — crossing allocators is the other classic way this boundary corrupts memory.
        ///
        /// # Safety
        /// `ptr`/`len` must be exactly what `juno_call` returned, freed once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn juno_free(ptr: *mut u8, len: usize) {
            if !ptr.is_null() {
                drop(unsafe { ::std::vec::Vec::from_raw_parts(ptr, len, len) });
            }
        }
    };
}

/// Serialise a response into a buffer the controller can read. Public because the macro
/// expands in the driver's crate; not meant to be called directly.
#[doc(hidden)]
pub fn __respond(response: Response, out_len: *mut usize) -> *mut u8 {
    let mut bytes = serde_json::to_vec(&response).unwrap_or_else(|_| b"{}".to_vec());
    bytes.shrink_to_fit();
    let len = bytes.len();
    let ptr = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    unsafe { *out_len = len };
    ptr
}
