//! `manifest.toml` — what a driver package declares about itself.
//!
//! The manifest is checked against the proxy registry before a driver is ever installed, so a
//! typo'd capability or a connection pointing at a proxy that does not exist fails at install
//! time with a list, not at 9pm in someone's living room.

use crate::proxy::{ProxyRegistry, Resolved};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Ids are driver-local and chosen by the driver author. They must be stable across versions:
/// a project remembers what it was bound to by this number.
pub use crate::LocalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Declarative,
    Python,
    Wasm,
    /// A compiled library the controller dlopens: `.dylib`, `.so`, `.dll`.
    ///
    /// Distinct from [`Runtime::Wasm`] because it is not sandboxed. A native plugin runs
    /// in-process with the controller's privileges, which is acceptable for drivers built by
    /// our own CI and is exactly why the registry marks anything else third-party. Calling
    /// these "wasm" — as every first-party manifest used to — meant the catalog advertised a
    /// sandbox that was not there.
    Native,
    /// A driver core registers itself, with no package behind it at all.
    ///
    /// The virtual devices `run --demo` builds are Rust structs handed straight to the runtime;
    /// there is no archive, no payload and nothing to load. They still need a manifest, because
    /// a device is bound through its declared proxies like any other — so they need a word for
    /// "there is no file", and `native` is not it. `native` means a dylib the package carries,
    /// which is what real drivers ship, and one runtime name meaning two things is a fault
    /// discovered at install time by somebody who did not write either.
    ///
    /// A package can never carry one. See [`crate::driver::package`].
    Builtin,
    /// A protocol stack in its own process, spoken to over a pipe. See
    /// [`crate::driver::adapter`], and [`AdapterDecl`] for the activation rule.
    ///
    /// The odd one out, because there is no in-process code at all: nothing is `dlopen`ed and
    /// nothing is interpreted here. The package *is* the tree the child process runs, which is
    /// why [`crate::driver::package`] asks it for no payload — and why a manifest saying
    /// `adapter` without an `[adapter]` table describes nothing that can start.
    Adapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Relay,
    Contact,
    IrOut,
    Serial,
}

impl ControlKind {
    /// The proxy a provider must implement to satisfy this kind of control connection.
    /// This mapping is the hinge of the whole abstraction: a driver asks for `relay` and gets
    /// bound to *any* binding of the `relay` proxy, whatever hardware is underneath.
    pub fn provider_proxy(self) -> &'static str {
        match self {
            ControlKind::Relay => "relay",
            ControlKind::Contact => "contact",
            ControlKind::IrOut => "ir_out",
            ControlKind::Serial => "serial_port",
        }
    }
}

/// Re-exported rather than defined here: a driver builds these to answer
/// [`crate::HostCall::Connections`], and `manifest` is behind the `contracts` feature that a
/// driver does not compile. They live in [`crate::host`], which is always built, and stay
/// reachable under their long-standing paths from here.
pub use crate::host::{ConnectionDecl, Direction};

fn default_true() -> bool {
    true
}
fn default_api() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriverMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub manufacturer: String,
    pub version: String,
    pub runtime: Runtime,
    #[serde(default = "default_api")]
    pub api: u32,
    pub min_core: Option<String>,
    /// Marks the driver a multi-driver package leads with — the one that names the artifact
    /// and that the installer reports.
    ///
    /// Only needed when a package's drivers are siblings, like a Roku TV and a Roku player:
    /// where one is another's `parent` the bridge leads on its own, and where there is a lone
    /// `manifest.toml` it leads by definition. Left unset everywhere, the choice would fall to
    /// alphabetical order, which is stable but says nothing.
    #[serde(default)]
    pub primary: bool,
    /// What the whole package is called, when that is not the lead driver's own name.
    ///
    /// A catalog lists *products*, and a product's name is not always a driver's. The Hue
    /// package leads with a driver called `Philips Hue Bridge`, which is the honest name for
    /// the driver and the wrong name for the shelf — nobody buys a bridge, they buy Philips
    /// Hue. TP-Link is worse: the lead driver is `TP-Link Account`, a cloud login, and the
    /// product on the box is Tapo.
    ///
    /// Unset means the lead driver's `name` is the product name, which is true of most
    /// packages — an Apple TV package leads with a driver called Apple TV.
    ///
    /// Only read on the driver a package leads with. On any other it says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// What this product *is*, as a device class — a proxy type name.
    ///
    /// Not what the lead driver implements: a Hue package leads with a bridge, and a bridge is
    /// how the lights are reached rather than what somebody bought. Declaring `kind = "light"`
    /// is what puts Philips Hue under Lighting with a bulb beside it instead of under System
    /// with a router.
    ///
    /// Validated against the proxy registry, so a typo fails at `junodrv check` rather than
    /// filing a product under a group that does not exist. Unset falls back to the proxy the
    /// driver leads with, which is right for anything that is the thing it controls — a
    /// television, a receiver — and wrong for every hub.
    ///
    /// Only read on the driver a package leads with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The same product as the driver named here, reached a different way.
    ///
    /// An Apple TV is a native driver over its Companion link and a `commands.toml` of IR codes
    /// over an emitter. That is one product with two ways in, not two products, and a catalog
    /// listing both as siblings asks somebody to choose between two nearly identical rows
    /// before they know there is a choice to make. With this the catalog carries one row, and
    /// the choice happens where it belongs — while adding it, where the answer changes the
    /// setup that follows.
    ///
    /// Names another driver in the same package. Discovery beats the choice outright: something
    /// heard on the network was heard *as* one of these, so that is the one that gets set up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    /// This driver can replace a logical group's per-device fan-out with one native request.
    ///
    /// The flag is the compatibility gate for [`crate::DriverModule::on_group`]. Core never
    /// sends that ABI request to an older driver unless its own manifest opts in, so adding the
    /// request does not turn an otherwise compatible package into one that cannot be loaded.
    #[serde(default)]
    pub group_control: bool,
    /// This driver can store and recall scenes on a bridge/controller.
    ///
    /// Like [`Self::group_control`], this is an additive ABI gate. Core only sends
    /// [`crate::Request::OnScene`] to packages that opt in, so older drivers continue to load
    /// without having to understand native scene requests.
    #[serde(default)]
    pub scene_control: bool,
    /// Driver id of the bridge these devices live behind, if they do.
    ///
    /// A child inherits its parent's properties, so a Hue bulb does not carry its own copy of
    /// the bridge address — it reads the one the bridge holds. That is the whole point: a
    /// bridge that moves to a new IP is edited once.
    pub parent: Option<String>,
    /// Which column of the app catalog this driver reads, when it launches apps at all.
    ///
    /// `roku`, `apple_tv`, `android_tv`, `fire_tv`, `webos`, `tizen` — the keys in
    /// [junohouse/apps](https://github.com/junohouse/apps). Core looks the requested app up
    /// there and passes `launch_id` on the `launch_app` command, so a driver does not carry its
    /// own table of channel numbers and bundle ids, and a correction reaches houses without any
    /// driver being rebuilt.
    ///
    /// Optional, and most drivers want nothing to do with it: a box that reports its own
    /// installed apps already knows better than any table, and should keep using what it read
    /// from the device. This is for the ones that cannot — a set whose local API has no endpoint
    /// listing what is installed — and as a fallback for an app a device did not mention.
    #[serde(default)]
    pub app_platform: Option<String>,
    /// How much of this driver's own output the controller keeps — and, by being here at all,
    /// that an installer is allowed to change it.
    ///
    /// A driver says plenty about itself: what it sent, what came back, what it made of a
    /// frame. Most of that is worth nothing in a working house and everything in a broken one,
    /// so the level it is held to has to be adjustable from the Logs page rather than fixed
    /// when the driver was built.
    ///
    /// Unset means the driver has not thought about it: its output is kept at `info` and the
    /// control is not offered, because a level control that a driver ignores is worse than no
    /// control at all. Set it to the level this driver is worth *when nothing is wrong* —
    /// almost always `info`, and `debug` only for something whose ordinary operation is what
    /// somebody is trying to see.
    ///
    /// This governs the driver's own lines, never the controller's account of it: a device
    /// that stops answering is core's observation and is reported whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LogLevel>,
}

/// What a driver is worth hearing about. Ordinary syslog levels, spelled as the rest of Juno
/// spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    /// Everything, including what the driver is doing between one command and the next. Never
    /// a manifest default: this is a level somebody turns on while watching.
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyDecl {
    pub id: LocalId,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
}

/// Something a driver can do that no proxy contract describes.
///
/// A proxy command is a promise about a *class* of device: every `light` answers `set_level`,
/// so an automation, the assistant and the UI can all speak to one without knowing what it is.
/// That is the whole value of the layer and it is why a driver cannot add to it.
///
/// But a Zigbee coordinator has to be told to open its network for sixty seconds, and a Z-Wave
/// controller has to be told to heal its mesh. Those are not commands on a *bridge* — they are
/// commands on *this* bridge, and inventing `bridge.permit_join` would put a Zigbee concept in
/// a contract that a Hue bridge and a Caséta bridge also have to satisfy.
///
/// So actions are the escape hatch, deliberately shaped so it cannot be mistaken for the other
/// thing: they are addressed by driver and device rather than by binding, they never appear in
/// a proxy contract, an automation cannot bind to one, and they are validated against *this
/// manifest* rather than against a shared contract. Somebody reading a project can always tell
/// which of the two they are looking at.
/// A driver that runs in its own process. See [`crate::driver::adapter`].
///
/// Its presence is the entire activation rule: core spawns this process when — and only when —
/// a device in the project uses a driver that declares one. There is no services page and
/// nothing to remember to turn off, because a settings screen that can disagree with reality
/// eventually will.
/// What a driver is allowed to grow at runtime.
///
/// `[[proxy]]` is what a driver *is*; this is what it may turn out to have behind it. The two are
/// different questions and only the first can be answered when the manifest is written: an alarm
/// panel has as many zones as somebody programmed into it, a Zigbee coordinator has whatever
/// joined the mesh last week.
///
/// It is a list rather than a flag because "this driver has children" is not a useful permission.
/// A driver that can present anything can present a `lock`, and then a bug in a vendor's firmware
/// parser — or a hostile answer from a device on the network — is a front door in somebody's
/// project that no installer put there. The list is written by the driver author, checked against
/// the registry at install, and enforced on every [`crate::HostCall::Present`] and every
/// `Up::Present`: a kind that is not in it is dropped with a warning, not adopted.
///
/// So it is not a formality. `["sensor", "security_partition"]` is a security panel saying what
/// it is for, and the day its driver tries to present a `lock` is the day somebody should be told
/// rather than the day a door appears.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildrenDecl {
    /// Proxy contracts this driver may present. Everything else is refused.
    #[serde(default)]
    pub proxies: Vec<String>,
    /// Adopt reported nodes immediately instead of waiting for an installer to select them.
    ///
    /// This is deliberately opt-in. It is appropriate for a commissioned system whose own
    /// room inventory is authoritative (for example Sonos), but not for an open radio network
    /// where hearing a device is not consent to add it to the project.
    #[serde(default)]
    pub auto_adopt: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDecl {
    /// `node`, or a binary shipped in the package.
    pub exec: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDecl {
    /// `permit_join`. Unique within the driver.
    pub name: String,
    /// What the button says. The driver's language, not ours.
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Arguments, in the order a form should show them.
    #[serde(default)]
    pub arg: Vec<ArgDecl>,
    /// Ask before running it. For anything that removes a device, rewrites a network key, or
    /// takes the mesh down for a minute.
    #[serde(default)]
    pub confirm: bool,
    /// Free the network for joining, erase a controller — the things worth a red button and a
    /// sentence explaining what is about to happen.
    #[serde(default)]
    pub danger: bool,
    /// Which of this driver's devices the action belongs to. See [`ActionOn`].
    #[serde(default)]
    pub on: ActionOn,
    /// Settings the device must have for this action to mean anything — **any one of them**.
    ///
    /// [`ActionOn`] narrows by *kind*; this narrows within a kind. An SNZB-06P and a door
    /// contact are both `sensor`, and only one of them has a presence hold — the difference is
    /// not in any contract and core cannot know it, so the adapter reports it per node in
    /// [`crate::adapter::Node::settings`] and this names what to look for.
    ///
    /// Any rather than all, and named so it cannot be read the other way: the list is almost
    /// always one knob under several vendors' spellings — `occupancy_timeout` on a presence
    /// sensor, `motion_timeout` on a PIR — and an action that needed both at once would apply
    /// to nothing. A driver that genuinely needs a conjunction does not exist yet; when one
    /// does it can have its own field rather than silently changing what this one means.
    ///
    /// Empty means the action applies to every device the scope allows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_one_of: Vec<String>,
}

/// Which of a driver's devices an action belongs to.
///
/// An action is a driver's, but it is never true of *all* a driver's devices, and treating it as
/// though it were is a bug the UI cannot correct for: one adapter manifest covers a Zigbee
/// coordinator and every node behind it, so a declaration with no scope offered "Allow devices to
/// join" on a battery sensor and "Hold presence for" on the radio. The driver refuses both, with
/// a reason — but a list of buttons that mostly do not work is not a list anybody reads.
///
/// This is the same rule proxy commands already follow: resolved per device, and a thing a device
/// cannot do does not appear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ActionOn {
    /// The device this manifest describes — a native driver's own device, or an adapter's
    /// coordinator. The default, because it is what a driver with one device means.
    #[default]
    Own,
    /// Any device the adapter surfaced behind it, whatever kind. `remove_node` is this: it makes
    /// sense for every node and for none of the coordinators.
    Node,
    /// Any of this driver's devices carrying a binding of this proxy type — `sensor`, `light`.
    /// Covers a node and a native driver's own device alike, since both have bindings.
    Proxy(String),
}

impl<'de> Deserialize<'de> for ActionOn {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ActionOn, D::Error> {
        // A plain string, so the manifest reads `on = "sensor"` rather than a tagged table for
        // what is always one word.
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "own" | "self" => ActionOn::Own,
            "node" => ActionOn::Node,
            _ => ActionOn::Proxy(s),
        })
    }
}

impl Serialize for ActionOn {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            ActionOn::Own => "own",
            ActionOn::Node => "node",
            ActionOn::Proxy(p) => p,
        })
    }
}

/// One argument of an [`ActionDecl`].
///
/// Nearly the same small vocabulary as `[[property]]`, so a driver author learns one thing and
/// a form renderer has one thing to render — and the exception is worth knowing. A property is
/// named by its `name`, which *is* the label, and explained by `tooltip`. An argument has a
/// separate `label`, because `name` is the key the driver receives and is not always something
/// to show somebody.
///
/// The two used to differ in a way that was simply a gap: an argument had nowhere to put the
/// sentence explaining it. A Zigbee adapter's `via` ended up with
/// `label = "Through — address of one router, or blank for the whole mesh"`, which is a tooltip
/// wearing a label's clothes and sixty characters wide in a row of controls.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArgDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    /// What to call it on screen. `name` is the key the driver is handed; this is for people.
    #[serde(default)]
    pub label: String,
    /// The sentence explaining it, shown on hover — the same job `PropertyDecl::tooltip` does.
    ///
    /// Keep it out of `label`. A label sits inline in a row of controls next to the button that
    /// runs the action, so a long one pushes everything else off the end; this has a tooltip to
    /// live in and no width to respect.
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub required: bool,
    pub default: Option<Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A closed set, rendered as a menu.
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlDecl {
    pub id: LocalId,
    pub kind: ControlKind,
    pub name: String,
    #[serde(default = "default_true")]
    pub required: bool,
    pub proxy: Option<LocalId>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportDecl {
    /// What this connection is, for whoever reads the manifest. Core dispatches on none of it:
    /// the socket is `port` plus `tls`, and a driver that wants MQTT says so by sending
    /// `HostCall::Publish`. Kept because a bare `[[transport]]` block says nothing at all.
    pub kind: String,
    /// The port core dials, for the drivers whose socket core owns — the ones that send
    /// `HostCall::Tx` or `HostCall::Publish`.
    ///
    /// Leave it unset for a driver that builds its own URLs through `HostCall::Http`: nothing
    /// reads it there, and a second copy of the port beside the one in the code is a copy that
    /// will disagree. Also unset when the port is announced rather than fixed — a Companion
    /// link and a HAP accessory pick one at boot and put it in their SRV record, which arrives
    /// as the device's own `Port` property and wins over this anyway.
    pub port: Option<u16>,
    /// Accepted and ignored. It named the mechanism that finds this hardware, which is what the
    /// `[discovery]` table does and has always been read from instead — nothing has ever
    /// dispatched on this. Kept only because `deny_unknown_fields` means removing it would stop
    /// every already-published package that writes one from installing; do not write new ones.
    #[serde(default)]
    pub discovery: String,
    #[serde(default)]
    pub keepalive: bool,
    /// This connection carries framed bytes, not lines of text.
    ///
    /// Set it and reads come back as `rx { bytes }` — a JSON array — with core doing no framing
    /// at all, because it cannot know where one message ends. Leave it unset and reads come back
    /// as `rx { data }`, line-oriented and UTF-8, which is right for telnet, for SSE and for
    /// every driver that has needed this so far.
    ///
    /// It has to be declared rather than inferred. A binary frame read down the text path is cut
    /// at the first `0x0A` inside its length prefix and has every non-UTF-8 byte replaced, and
    /// neither is detectable afterwards — the driver receives something that decodes to
    /// plausible nonsense. Guessing wrong in that direction is worse than saying so here.
    #[serde(default)]
    pub binary: bool,
    /// Wrap the connection in mutual TLS — a client certificate property must be set on the
    /// device (or inherited from its bridge) or the connection is refused.
    #[serde(default)]
    pub tls: bool,
    /// Fixed MQTT CONNECT credentials, for a broker that wants a password but not a per-install
    /// secret — the whole product line shares one, published in the vendor's own docs. Unlike
    /// `[[property]]`, this is not something an installer sets or a pairing flow discovers: it
    /// is the same string for every unit, so it belongs on the manifest next to `port`, not on
    /// the device.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// How to recognize this hardware on a network it does not announce itself on.
    ///
    /// Set it and a controller with this driver *installed* sweeps its own network for `port`
    /// when a survey runs, so the address does not have to be found and typed in. Leave it
    /// unset and nothing is swept — see [`crate::manifest::Probe`] for why this is opt-in and
    /// why it only ever applies to installed drivers.
    pub probe: Option<Probe>,
}

/// What to send to something that might be this driver's hardware, and what proves it is.
///
/// The point is to be *sure*. An open port says only that something is listening on the number
/// this driver expects, which on a busy network is a coin toss; a reply that could only have
/// come from the right software is an identification. So the exchange wants to be one that
/// needs no credentials and grants nothing — a refusal is ideal, because being told to go away
/// in the right dialect proves who is talking.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    /// Sent on connect. Nothing is sent if this is absent, and an open port is the whole claim.
    pub send: Option<String>,
    /// Confirmed if the reply contains this. Matched as a plain substring, not a pattern:
    /// a discovery rule is read by whoever is wiring the house up, and a regex in a manifest
    /// is a second language to learn to answer "would this find my box".
    pub expect: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub tooltip: String,
    pub default: Option<Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub unit: String,
    /// The device cannot work until somebody sets this.
    ///
    /// For the ones with no sensible default and no way to discover them — a Sonos API key, an
    /// account name a driver cannot invent. Distinct from merely having no `default`: plenty
    /// of properties are optional and blank, and a screen that flagged all of them would flag
    /// nothing.
    ///
    /// Advisory, and the driver still checks. This says what an installer must be told, not
    /// what the controller enforces — a driver that assumes core blocked an empty value is one
    /// that panics the day somebody edits the project file by hand.
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// A bare service type, or a table naming TXT keys the device must carry — see
    /// [`crate::mdns::MdnsMatch`].
    #[serde(default)]
    pub mdns: Vec<crate::mdns::MdnsMatch>,
    #[serde(default)]
    pub ssdp: Vec<String>,
    /// SDDP is matched across several fields at once, so an entry is either a bare type
    /// string or a table — see [`crate::driver::catalog::SddpMatch`].
    #[serde(default)]
    pub sddp: Vec<crate::sddp::SddpMatch>,
    #[serde(default)]
    pub mac_oui: Vec<String>,
    /// Broadcast a vendor's own discovery query and claim what answers — see
    /// [`crate::udp::UdpMatch`]. For hardware that speaks none of the three standards above,
    /// which is most of what a house actually contains.
    #[serde(default)]
    pub udp: Vec<crate::udp::UdpMatch>,
    /// What answers these rules is not this driver — it is one of the devices behind it.
    ///
    /// For a bridge whose *children* are what appear on the network while the bridge itself
    /// appears nowhere. A TP-Link account is the case that forced it: a Tapo dimmer answers
    /// the discovery broadcast, and the account answers nothing at all, because an account is
    /// not a thing with an address. So the rules that find a dimmer have to live on the
    /// account — that is the driver a controller must install first, and the thing that has to
    /// be set up before any dimmer can work — while what they actually find is a dimmer.
    ///
    /// Core reads it as a two-stage answer to the same sighting: with no such bridge in the
    /// project, the find offers **this** driver, because nothing can be added until it exists.
    /// Once one is set up, the same find offers the driver named here instead, adopted behind
    /// it. Which is what somebody means by "discovery found the hub, and after that it finds
    /// the lights."
    ///
    /// Only for a bridge that is genuinely invisible. A Hue bridge answers `_hue._tcp` *as
    /// itself*, so its rules stay its own and this stays unset — declaring it there would turn
    /// a bridge already in the project into a standing offer to add a bulb at the bridge's own
    /// address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt_as: Option<String>,
    // There was an `http` matcher here — a path to fetch and a string the body had to contain.
    // Nothing ever read it: no controller implemented it and no manifest declared one, so it
    // was a field the docs promised and the network never acted on. Asking a device a question
    // to identify it is what `[[transport]] probe` does, and that one works.
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub driver: DriverMeta,
    #[serde(default)]
    pub proxy: Vec<ProxyDecl>,
    #[serde(default)]
    pub control: Vec<ControlDecl>,
    #[serde(default)]
    pub connection: Vec<ConnectionDecl>,
    #[serde(default)]
    pub transport: Vec<TransportDecl>,
    #[serde(default)]
    pub property: Vec<PropertyDecl>,
    #[serde(default)]
    pub discovery: Discovery,
    /// Driver-specific actions. See [`ActionDecl`] for why these are not proxy commands.
    #[serde(default)]
    pub action: Vec<ActionDecl>,
    /// Present if this driver is a separate process. See [`AdapterDecl`].
    #[serde(default)]
    pub adapter: Option<AdapterDecl>,
    /// What this driver may find behind it at runtime. See [`ChildrenDecl`].
    #[serde(default)]
    pub children: Option<ChildrenDecl>,
}

/// Reject a driver id that could escape the directory it names.
///
/// Lowercase alphanumerics, dots and underscores. Not a style rule — the registry index schema
/// handles house style for anything certified, and a sideloaded `.junodrv` never goes through
/// CI. This is only about what can safely become a path.
fn validate_driver_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() {
        return Err("it is empty");
    }
    if id.len() > 128 {
        return Err("it is longer than 128 characters");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
    {
        return Err("only lowercase letters, digits, `.` and `_` are allowed");
    }
    // `..` is the traversal, and a leading or trailing dot is how it gets smuggled past a
    // check that only looks for the pair.
    if id.contains("..") || id.starts_with('.') || id.ends_with('.') {
        return Err("it may not contain `..` or begin or end with a dot");
    }
    // Deliberately NOT requiring two segments. Reverse-DNS is the convention and the registry
    // index schema enforces it for anything certified, but a single-segment id cannot traverse
    // anywhere, and this function's job is the filesystem, not house style.
    Ok(())
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Self, toml::de::Error> {
        let m: Manifest = toml::from_str(src)?;
        // A driver id reaches the filesystem: it names the unpacked plugin directory and the
        // archive an upload is written to. `Path::join` with an absolute string *replaces* the
        // base rather than appending to it, so an id of `/etc/cron.d/x` escapes the drivers
        // directory entirely — and `../` gets there the slower way. Validated here, at the one
        // place every manifest is parsed, rather than at each place an id becomes a path,
        // because the next such place will forget.
        if let Err(why) = validate_driver_id(&m.driver.id) {
            return Err(serde::de::Error::custom(format!(
                "driver.id `{}` is not usable: {why}",
                m.driver.id
            )));
        }
        Ok(m)
    }

    /// Whether this driver's devices must be attached to a bridge.
    pub fn needs_parent(&self) -> Option<&str> {
        self.driver.parent.as_deref()
    }

    /// Whether this driver may present a node of this kind. See [`ChildrenDecl`].
    ///
    /// A driver with no `[children]` block may present nothing at all, which is the right default
    /// for the overwhelming majority: a television has no children, and a driver that grew some
    /// by accident is a bug worth hearing about rather than a feature worth allowing.
    pub fn may_present(&self, kind: &str) -> bool {
        self.children
            .as_ref()
            .is_some_and(|c| c.proxies.iter().any(|p| p == kind))
    }

    /// Whether newly presented nodes should be adopted without an installer selection.
    pub fn auto_adopts_children(&self) -> bool {
        self.children.as_ref().is_some_and(|c| c.auto_adopt)
    }

    /// The proxy the driver leads with — explicit `primary`, else the first declared.
    pub fn primary_proxy(&self) -> Option<LocalId> {
        self.proxy
            .iter()
            .find(|p| p.primary)
            .or_else(|| self.proxy.first())
            .map(|p| p.id)
    }

    /// What this product is, as a device class — declared [`DriverMeta::kind`], else the proxy
    /// type the driver leads with.
    ///
    /// The fallback is right for a device that is the thing it controls and wrong for every
    /// hub, which is exactly why the field exists. It is here so a package written before the
    /// field did lands somewhere rather than nowhere.
    pub fn kind(&self) -> Option<&str> {
        self.driver.kind.as_deref().or_else(|| {
            self.proxy
                .iter()
                .find(|p| p.primary)
                .or_else(|| self.proxy.first())
                .map(|p| p.ty.as_str())
        })
    }

    /// What a catalog calls this package — declared [`DriverMeta::product`], else its own name.
    pub fn product(&self) -> &str {
        self.driver.product.as_deref().unwrap_or(&self.driver.name)
    }

    pub fn control(&self, id: LocalId) -> Option<&ControlDecl> {
        self.control.iter().find(|c| c.id == id)
    }

    /// Every problem at once — a driver author wants the whole list, not the first line.
    /// The action with this name, if the driver declared one.
    pub fn action(&self, name: &str) -> Option<&ActionDecl> {
        self.action.iter().find(|a| a.name == name)
    }

    pub fn validate(&self, registry: &ProxyRegistry) -> Vec<String> {
        let mut errs = Vec::new();

        // An adapter and its `[adapter]` table imply each other, and each without the other
        // describes something that cannot start. Refused here — the one place every manifest goes
        // through — rather than at spawn, where the symptom is a driver that installs and then
        // never answers.
        match (self.driver.runtime, self.adapter.is_some()) {
            // The only two coherent shapes, and they are the first arms so neither falls through
            // into the complaints below.
            (Runtime::Adapter, true) => {}
            (Runtime::Adapter, false) => errs.push(
                "runtime is `adapter` but there is no [adapter] table saying what to run".into(),
            ),
            (other, true) => errs.push(format!(
                "there is an [adapter] table but runtime is `{other:?}` — \
                 an adapter's code runs in its own process, so set runtime = \"adapter\""
            )),
            _ => {}
        }

        // Actions are validated against this manifest rather than a shared contract, so this is
        // the only place a mistake in one can be caught. A driver whose action shadows a proxy
        // command is the case worth being strict about: the two dispatch differently, and a
        // person reading "set_level" on a device should never have to work out which they got.
        let mut action_names = BTreeSet::new();
        for a in &self.action {
            if !action_names.insert(a.name.as_str()) {
                errs.push(format!("duplicate action `{}`", a.name));
            }
            if a.name.is_empty() || a.label.is_empty() {
                errs.push(format!("action `{}` needs a name and a label", a.name));
            }
            for p in &self.proxy {
                if let Some(proxy) = registry.get(&p.ty)
                    && proxy.commands.contains_key(&a.name)
                {
                    errs.push(format!(
                        "action `{}` has the same name as a command on the `{}` proxy — one of \
                         them has to change, or nobody can tell which is being invoked",
                        a.name, p.ty
                    ));
                }
            }
            let mut arg_names = BTreeSet::new();
            for arg in &a.arg {
                if !arg_names.insert(arg.name.as_str()) {
                    errs.push(format!("action `{}` declares `{}` twice", a.name, arg.name));
                }
                if !matches!(arg.ty.as_str(), "string" | "number" | "bool" | "password") {
                    errs.push(format!(
                        "action `{}` argument `{}` has unknown type `{}`",
                        a.name, arg.name, arg.ty
                    ));
                }
            }
        }

        if self.proxy.is_empty() {
            errs.push("driver declares no [[proxy]] — it would control nothing".into());
        }

        // Checked here rather than when a node arrives, because the failure this prevents is a
        // typo: `[children] proxies = ["sensors"]` refuses every zone the panel offers, at nine
        // at night, with nothing in the log but "not allowed to present `sensor`".
        if let Some(children) = &self.children {
            if children.proxies.is_empty() {
                errs.push(
                    "[children] lists no proxies — remove the block, or say what may be presented"
                        .into(),
                );
            }
            for kind in &children.proxies {
                if registry.get(kind).is_none() {
                    errs.push(format!("[children]: `{kind}` is not a proxy in this core"));
                }
            }
        }

        let mut seen = BTreeSet::new();
        for p in &self.proxy {
            if !seen.insert(p.id) {
                errs.push(format!("duplicate proxy id {}", p.id));
            }
            match registry.get(&p.ty) {
                None => errs.push(format!("proxy {}: unknown type `{}`", p.id, p.ty)),
                Some(proxy) => {
                    if let Err(e) = proxy.resolve(&p.capabilities) {
                        for msg in e {
                            errs.push(format!("proxy {}: {msg}", p.id));
                        }
                    }
                }
            }
        }
        if self.proxy.iter().filter(|p| p.primary).count() > 1 {
            errs.push("more than one proxy marked primary".into());
        }

        // A typo here is otherwise silent: the product files itself under a group nothing else
        // is in, and nobody notices until somebody goes looking for it on the shelf.
        if let Some(kind) = &self.driver.kind
            && registry.get(kind).is_none()
        {
            errs.push(format!("driver kind: `{kind}` is not a proxy in this core"));
        }
        if self.driver.variant_of.as_deref() == Some(self.driver.id.as_str()) {
            errs.push("driver variant_of names this driver itself".into());
        }

        let mut seen = BTreeSet::new();
        for c in &self.control {
            if !seen.insert(c.id) {
                errs.push(format!("duplicate control id {}", c.id));
            }
            if registry.get(c.kind.provider_proxy()).is_none() {
                errs.push(format!(
                    "control {}: no `{}` proxy in this core",
                    c.id,
                    c.kind.provider_proxy()
                ));
            }
            if let Some(p) = c.proxy
                && !self.proxy.iter().any(|d| d.id == p)
            {
                errs.push(format!("control {}: proxy {p} is not declared", c.id));
            }
        }

        let mut seen = BTreeSet::new();
        for c in &self.connection {
            if !seen.insert(c.id) {
                errs.push(format!("duplicate connection id {}", c.id));
            }
            if let Some(proxy) = c.proxy
                && !self.proxy.iter().any(|d| d.id == proxy)
            {
                errs.push(format!(
                    "connection {}: proxy {proxy} is not declared",
                    c.id
                ));
            }
        }

        // A discovery payload that will not decode is the failure this catches best: the hex
        // is written by hand from a packet capture, an odd digit or a stray character is easy,
        // and the symptom on a controller is a broadcast that goes out short or not at all and
        // a driver that quietly never finds anything.
        for rule in &self.discovery.udp {
            if rule.port == 0 {
                errs.push("discovery.udp: a rule needs a port to broadcast to".into());
            }
            if rule.payload().is_none() {
                errs.push(format!(
                    "discovery.udp port {}: `send_hex` is not hex — pairs of hex digits,                      optionally separated by spaces, `:` or `-`",
                    rule.port
                ));
            }
        }

        errs
    }

    /// Resolve every declared proxy against the registry. Call after [`Self::validate`].
    pub fn resolve_all(
        &self,
        registry: &ProxyRegistry,
    ) -> Result<BTreeMap<LocalId, Resolved>, Vec<String>> {
        let mut out = BTreeMap::new();
        let mut errs = Vec::new();
        for p in &self.proxy {
            match registry.get(&p.ty) {
                None => errs.push(format!("unknown proxy type `{}`", p.ty)),
                Some(proxy) => match proxy.resolve(&p.capabilities) {
                    Ok(r) => {
                        out.insert(p.id, r);
                    }
                    Err(e) => errs.extend(e),
                },
            }
        }
        if errs.is_empty() { Ok(out) } else { Err(errs) }
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    fn manifest_with(id: &str) -> Result<Manifest, toml::de::Error> {
        Manifest::parse(&format!(
            r#"
            [driver]
            id = "{id}"
            name = "X"
            version = "1.0.0"
            runtime = "native"
            [[proxy]]
            id = 1
            type = "light"
            "#
        ))
    }

    /// A driver id names a directory and an uploaded archive. `Path::join` with an absolute
    /// string discards the base, so this is the difference between writing inside the drivers
    /// directory and writing anywhere on the disk.
    #[test]
    fn an_id_cannot_walk_out_of_the_drivers_directory() {
        for hostile in [
            "../../../etc/cron.d/evil",
            "/etc/cron.d/evil",
            "roku/../../..",
            "..",
            "a..b",
            ".hidden",
            "trailing.",
        ] {
            assert!(
                manifest_with(hostile).is_err(),
                "`{hostile}` was accepted as a driver id"
            );
        }
    }

    /// A probe's `send` goes onto a socket byte for byte, so whether TOML treated `\n` as an
    /// escape or as two characters decides whether the far side ever sees a complete line.
    /// A driver that declares a probe in a literal string finds nothing, silently, for ever.
    #[test]
    fn a_probe_line_ends_in_a_real_newline() {
        let m: Manifest = toml::from_str(
            r#"
            [driver]
            id = "juno.control4"
            name = "Control4"
            version = "1.0.0"
            runtime = "adapter"

            [adapter]
            exec = "juno-control4"

            [[proxy]]
            id = 1
            type = "bridge"

            [[transport]]
            kind = "network"
            port = 7421

            [transport.probe]
            send = "{\"op\":\"hello\",\"token\":\"\"}\n"
            expect = "\"op\":\"denied\""
            "#,
        )
        .expect("manifest with a probe should parse");

        let probe = m.transport[0]
            .probe
            .as_ref()
            .expect("probe should be there");
        let send = probe.send.as_deref().unwrap();
        assert!(
            send.ends_with('\n'),
            "probe line must end in a newline: {send:?}"
        );
        assert!(
            send.contains(r#""op":"hello""#),
            "quotes should be real: {send:?}"
        );
        assert_eq!(probe.expect.as_deref(), Some(r#""op":"denied""#));
    }

    /// A transport without one is the normal case, and must stay the normal case: a missing
    /// `[transport.probe]` is what stops a controller sweeping the network for every driver.
    #[test]
    fn a_transport_without_a_probe_sweeps_nothing() {
        let m = manifest_with("roku.tv").unwrap();
        assert!(m.transport.iter().all(|t| t.probe.is_none()));
    }

    /// Fixed MQTT credentials belong on the manifest — the whole product line shares one
    /// broker password — and have to survive a round trip through TOML same as `port` does.
    #[test]
    fn mqtt_credentials_on_a_transport_parse() {
        let m: Manifest = toml::from_str(
            r#"
            [driver]
            id = "test.tv"
            name = "TV"
            version = "1.0.0"
            runtime = "wasm"
            [[proxy]]
            id = 1
            type = "media_player"
            [[transport]]
            kind = "mqtt"
            port = 36669
            tls = true
            username = "hisenseservice"
            password = "multimqttservice"
            "#,
        )
        .expect("manifest with mqtt credentials should parse");
        let t = &m.transport[0];
        assert_eq!(t.username.as_deref(), Some("hisenseservice"));
        assert_eq!(t.password.as_deref(), Some("multimqttservice"));

        // Absent for every transport that has never needed one — a Roku, a Denon.
        let plain = manifest_with("roku.tv").unwrap();
        assert!(
            plain
                .transport
                .iter()
                .all(|t| t.username.is_none() && t.password.is_none())
        );
    }

    /// The default has to be "nothing", and a typo in the list has to be caught while somebody is
    /// still looking at the manifest. Both, because they are the two ways this fails: silently
    /// allowing everything, or silently allowing nothing at nine at night.
    #[test]
    fn a_driver_may_present_only_what_it_declared() {
        let registry = crate::proxy::ProxyRegistry::bundled().expect("contracts load");

        let plain = manifest_with("test.tv").unwrap();
        assert!(
            !plain.may_present("light"),
            "a driver with no [children] may present nothing"
        );

        let hub = Manifest::parse(
            r#"
            [driver]
            id = "test.panel"
            name = "Panel"
            version = "1.0.0"
            runtime = "native"
            [[proxy]]
            id = 1
            type = "bridge"
            [children]
            proxies = ["sensor", "security_partition"]
            "#,
        )
        .expect("parses");
        assert!(hub.may_present("sensor"));
        assert!(!hub.may_present("lock"), "a lock is not on the list");
        assert!(!hub.auto_adopts_children(), "adoption is opt-in");
        assert!(hub.validate(&registry).is_empty());

        let commissioned = Manifest::parse(
            r#"
            [driver]
            id = "test.commissioned"
            name = "Commissioned system"
            version = "1.0.0"
            runtime = "native"
            [[proxy]]
            id = 1
            type = "bridge"
            [children]
            proxies = ["sensor"]
            auto_adopt = true
            "#,
        )
        .expect("parses");
        assert!(commissioned.auto_adopts_children());

        let typo = Manifest::parse(
            r#"
            [driver]
            id = "test.panel"
            name = "Panel"
            version = "1.0.0"
            runtime = "native"
            [[proxy]]
            id = 1
            type = "bridge"
            [children]
            proxies = ["sensors"]
            "#,
        )
        .expect("parses");
        assert!(
            typo.validate(&registry)
                .iter()
                .any(|e| e.contains("sensors")),
            "a misspelled kind must be refused at install, not at the first inventory"
        );

        let empty = Manifest::parse(
            r#"
            [driver]
            id = "test.panel"
            name = "Panel"
            version = "1.0.0"
            runtime = "native"
            [[proxy]]
            id = 1
            type = "bridge"
            [children]
            proxies = []
            "#,
        )
        .expect("parses");
        assert!(
            !empty.validate(&registry).is_empty(),
            "an empty list says nothing"
        );
    }

    #[test]
    fn ordinary_ids_still_parse() {
        for good in [
            "roku.tv",
            "signify.hue.bridge",
            "lutron.caseta.leap_dimmer",
            "a.b",
        ] {
            assert!(manifest_with(good).is_ok(), "`{good}` was rejected");
        }
    }

    /// Uppercase and spaces are not a traversal, but they make an id that is one thing on a
    /// case-sensitive filesystem and another on a Mac.
    #[test]
    fn ids_are_lowercase_and_free_of_whitespace() {
        assert!(manifest_with("Roku.TV").is_err());
        assert!(manifest_with("roku tv").is_err());
        // One segment is unconventional and perfectly safe, so it parses. The registry
        // schema is where reverse-DNS is insisted upon.
        assert!(manifest_with("roku").is_ok());
    }
}

impl ActionDecl {
    /// Check arguments before the driver sees them.
    ///
    /// The same job `Proxy::validate_call` does for commands, done here because an action has no
    /// contract to be checked against. Every problem at once rather than the first, because the
    /// caller is usually a form and a person would rather fix three fields in one go.
    pub fn validate_args(&self, args: &BTreeMap<String, Value>) -> Result<(), String> {
        let mut errs = Vec::new();

        for arg in &self.arg {
            let Some(value) = args.get(&arg.name) else {
                if arg.required && arg.default.is_none() {
                    errs.push(format!("`{}` is required", arg.name));
                }
                continue;
            };
            let ok = match arg.ty.as_str() {
                "string" | "password" => value.is_string(),
                "number" => value.is_number(),
                "bool" => value.is_boolean(),
                _ => true,
            };
            if !ok {
                errs.push(format!("`{}` should be a {}", arg.name, arg.ty));
                continue;
            }
            if let Some(n) = value.as_f64() {
                if arg.min.is_some_and(|m| n < m) {
                    errs.push(format!(
                        "`{}` is below the minimum of {}",
                        arg.name,
                        arg.min.unwrap()
                    ));
                }
                if arg.max.is_some_and(|m| n > m) {
                    errs.push(format!(
                        "`{}` is above the maximum of {}",
                        arg.name,
                        arg.max.unwrap()
                    ));
                }
            }
            if !arg.values.is_empty()
                && value
                    .as_str()
                    .is_some_and(|s| !arg.values.iter().any(|v| v == s))
            {
                errs.push(format!(
                    "`{}` must be one of: {}",
                    arg.name,
                    arg.values.join(", ")
                ));
            }
        }

        // An argument nobody declared is a typo in a form or a driver and a caller that failed
        // silently is how it survives to production.
        for name in args.keys() {
            if !self.arg.iter().any(|a| &a.name == name) {
                errs.push(format!("`{name}` is not an argument of `{}`", self.name));
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }
}
