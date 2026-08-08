# driver-sdk

The contract between a [Juno](https://juno.house) controller and a driver: the types both
sides speak, the proxy contracts a driver implements, the manifest and package formats, and
`junodrv` to check and build one.

Public, and depends on nothing of Juno's — writing a driver needs access to this repository
and nothing else.

**Everything you need to write one is at [docs.juno.house](https://docs.juno.house).**

- [Start here](https://docs.juno.house/) — a driver end to end
- [Connections](https://docs.juno.house/connections) — relays, contacts, IR, serial
- [Runtimes](https://docs.juno.house/runtimes) — declarative, Python, WASM, and the host ABI
- [Manifest reference](https://docs.juno.house/manifest) — what a package declares
- [Proxy reference](https://docs.juno.house/proxies/) — every device class, generated from
  `proxies/*.toml` in this repo

```toml
[dependencies]
driver-sdk = { git = "https://github.com/junohouse/driver-sdk", tag = "v0.7.0" }

# A driver is downloaded and kept per project, and it spends its time waiting on a device
# rather than in its own code — so build it for size. Worth 30-35% on every certified driver
# measured, with no source change.
#
# `panic = "abort"` is deliberately not here. `export_driver!` wraps dispatch in `catch_unwind`
# so a driver that panics reports a warning instead of taking the controller down with it;
# aborting trades that for a few KB and makes one bad driver everybody's outage.
[profile.release]
opt-level = "z"
codegen-units = 1
lto = true
strip = true
```

About 40% of what is left is Rust's own std, statically linked into every cdylib — that floor
is the price of a driver being a separate loadable library. The way to get under it is not to
link a dependency at all: anything a driver needs a real crate for, ask for it as a host call
instead. The controller already has the crate, and `HostCall` is the shared-library mechanism
this ABI actually has.

## License

MIT OR Apache-2.0.
