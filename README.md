# juno-driver-sdk

Write a driver for a [Juno](https://juno.house) controller.

```toml
[dependencies]
juno-driver-sdk = { git = "https://github.com/junohouse/juno-driver-sdk", tag = "v0.1.0" }
serde_json = "1"

[lib]
crate-type = ["cdylib"]
```

```rust
use juno_driver_sdk::*;

#[derive(Default)]
struct MyLight;

impl DriverModule for MyLight {
    fn on_command(&self, inst: &mut Instance, proxy: LocalId, cmd: &str, args: &Args)
        -> Vec<HostCall>
    {
        match cmd {
            "on"  => vec![HostCall::Http(HttpRequest::new("POST", "…"))],
            other => vec![HostCall::warn(format!("unhandled `{other}`"))],
        }
    }
}

export_driver!(MyLight);
```

`cargo build --release` produces the driver; `junod pack` wraps it with a manifest into a
`.junodrv` a controller installs.

## This crate depends on nothing of Juno's

Only `serde` and `serde_json`. It is the *contract* between a controller and a driver, not an
excerpt of the controller — so it owns the types on both sides of it.

That matters practically: it used to re-export the controller's own types, which meant every
driver compiled the whole controller, from a private repository, to see three structs. Nobody
outside could build a driver at all. Now the controller depends on this crate, drivers depend
on this crate, and writing one needs access to nothing.

## The boundary is JSON

A driver never shares a Rust type with the controller. Rust has no stable ABI, so passing a
`Box<dyn DriverModule>` across a `dylib` boundary works right up until the two are built by
different compiler versions, at which point it corrupts memory silently. Serialising the call
costs microseconds and cannot do that — and it is exactly the boundary a WASM runtime needs,
so drivers will not change shape when sandboxing arrives.

`export_driver!` exports the three C entry points a controller looks for. You do not call
them; they exist so `unsafe` lives here rather than in every driver.

## Versioning

`ABI_VERSION` is checked when a driver is loaded — a driver built against a different one is
refused rather than half-understood. Pin the tag, and the check is a backstop rather than
something you meet.

## What a driver may do

Return [`HostCall`]s. The controller performs the I/O: HTTP, raw sockets, LEAP, HomeKit. A
driver decides *what* should happen and never opens a socket itself, which is what makes the
same driver code work behind a sandbox later.

## License

MIT OR Apache-2.0.
