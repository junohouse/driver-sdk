# driver-sdk

The contract between a [Juno](https://juno.house) controller and a driver: the types both
sides speak, the proxy contracts a driver implements, the manifest and package formats, and
`junodrv` to check and build one.

Public, and depends on nothing of Juno's — writing a driver needs access to this repository
and nothing else.

**Everything you need to write one is at [docs.juno.house](https://docs.juno.house).**

- [Start here](https://docs.juno.house/) — a driver end to end
- [Connections](https://docs.juno.house/connections) — relays, contacts, IR, serial
- [Runtimes](https://docs.juno.house/runtimes) — declarative, WASM, and the host ABI
- [Manifest reference](https://docs.juno.house/manifest) — what a package declares
- [Proxy reference](https://docs.juno.house/proxies/) — every device class, generated from
  `proxies/*.toml` in this repo

```toml
[dependencies]
# Track `main`. This repository carries no tags on purpose: the contracts are still moving, and
# a pin that has to be bumped by hand is a pin that ends up years behind the contracts it
# claims to check. Your `Cargo.lock` records the exact commit.
driver-sdk = { git = "https://github.com/junohouse/driver-sdk", branch = "main" }

[lib]
crate-type = ["cdylib"]

# A driver is downloaded and kept per project, and it spends its time waiting on a device
# rather than in its own code — so build it for size. Worth 30-35% on every certified driver
# measured, with no source change.
[profile.release]
opt-level = "z"
codegen-units = 1
lto = true
strip = true
```

```bash
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
```

One artifact, every controller. A driver used to be built as a native shared library for each
platform a controller might be, which meant three builds, three files in the package, and a
fourth failure mode nobody could see coming — a `.so` built against one glibc will not load
against an older one, whatever its architecture says. `driver.wasm` has no architecture, no
libc and no symbol versioning.

It is also a sandbox. Your driver cannot open a file or a socket, read a clock, or see another
driver: the controller grants five imports, which is what Rust's std needs to start, report a
panic, and ask for entropy. This costs nothing in practice, because every side effect a driver
has was already a `HostCall` it returns for the controller to perform. What it buys is that a
driver need not be code the controller's owner has any reason to trust.

About 40% of a built module is Rust's own std — that floor is the price of a driver being
separately compiled. The way to get under it is not to link a dependency at all: anything a
driver needs a real crate for, ask for it as a host call instead. The controller already has
the crate, and `HostCall` is the shared-library mechanism this ABI actually has.

## License

MIT OR Apache-2.0.
