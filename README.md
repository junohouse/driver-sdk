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
driver-sdk = { git = "https://github.com/junohouse/driver-sdk", tag = "v0.5.0" }
```

## License

MIT OR Apache-2.0.
