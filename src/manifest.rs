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
    /// No code at all: the driver *is* a decode table, and core reads it as data.
    ///
    /// A mesh device is not something a module drives. Its frames arrive already decoded — the
    /// coordinator hands core a cluster and a payload, and `zigbee/<driver id>.json` says what
    /// that means — so a module here would have nothing to be called with. The T2i is the case:
    /// every line of its plugin existed to frame bytes off a USB port that a radio remote never
    /// uses, and once the port went there was no code left to ship.
    ///
    /// The payload is the table, and the package is refused without one, exactly as a `wasm`
    /// package is refused without its module.
    Zigbee,
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
    // ---- patched. The installer wires this to a port on another device, and the project
    // remembers which — see `ControlLink`. Each one names the proxy a provider must implement.
    Relay,
    Contact,
    IrOut,
    Serial,
    // ---- dialled. Core opens this itself, at the address the device carries. Nothing is
    // wired and nothing is persisted; what varies is the port and how the bytes are framed.
    Network,
    Tcp,
    Mqtt,
    /// HomeKit Accessory Protocol, which is a socket with its own handshake on top.
    Hap,
    /// Joined to a mesh and adopted under a coordinator. Neither patched nor dialled: the
    /// radio is somebody else's device and the frames arrive decoded — see the `zigbee/`
    /// table a package carries and [`Discovery::zigbee`], which is what finds one.
    Zigbee,
}

impl ControlKind {
    /// The proxy a provider must implement to satisfy this connection, for the kinds that are
    /// patched to one. `None` for the kinds core reaches itself.
    ///
    /// This mapping is the hinge of the whole abstraction: a driver asks for `ir_out` and gets
    /// bound to *any* binding of the `ir_out` proxy, whatever hardware is underneath.
    pub fn provider_proxy(self) -> Option<&'static str> {
        match self {
            ControlKind::Relay => Some("relay"),
            ControlKind::Contact => Some("contact"),
            ControlKind::IrOut => Some("ir_out"),
            ControlKind::Serial => Some("serial_port"),
            _ => None,
        }
    }

    /// The word the manifest is written with.
    ///
    /// Not `format!("{self:?}")`: `IrOut` debugs as `IrOut`, which lowercases to `irout` — a
    /// spelling no manifest uses, nothing matches on, and which reached the catalog before
    /// anybody noticed. The serde rename is the truth, so this repeats it once rather than
    /// letting every caller re-derive it.
    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::Relay => "relay",
            ControlKind::Contact => "contact",
            ControlKind::IrOut => "ir_out",
            ControlKind::Serial => "serial",
            ControlKind::Network => "network",
            ControlKind::Tcp => "tcp",
            ControlKind::Mqtt => "mqtt",
            ControlKind::Hap => "hap",
            ControlKind::Zigbee => "zigbee",
        }
    }

    /// Whether an installer has to wire this to something before the driver can work.
    pub fn is_patched(self) -> bool {
        self.provider_proxy().is_some()
    }

    /// Whether core opens this itself, at the device's own address.
    pub fn is_dialled(self) -> bool {
        matches!(
            self,
            ControlKind::Network | ControlKind::Tcp | ControlKind::Mqtt | ControlKind::Hap
        )
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
    /// Whether this is core's own plumbing rather than something somebody adds.
    ///
    /// A catalog that lists a light group beside Philips Hue is describing two kinds of thing in
    /// one list, so the browser keeps them apart — and it worked out which was which from
    /// `runtime = "builtin"`, which is a fact about *how a driver ships* and not about what it
    /// is. That held while every built-in was infrastructure. `juno.input` is the exception it
    /// was always going to meet: a games console on an HDMI lead is the most-added device in a
    /// real house and ships inside core because it has no protocol to package.
    ///
    /// Absent means "decide from the runtime", which is what every driver written before this
    /// gets and what every packaged driver wants. Set it only where the two disagree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
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
    /// What else this same endpoint can be, when the hardware is one port and two contracts.
    ///
    /// A 3.5 mm jack on a controller is an IR emitter or a serial lead depending on what
    /// somebody plugged into it, and nothing electrical decides which — the installer does,
    /// once, when they wire the rack. Without this the driver has to declare two bindings for
    /// one socket and hope only one is ever used, which puts a port in the house that does not
    /// exist and lets a room be routed through it.
    ///
    /// `type` is what it is until somebody says otherwise. The alternates are the rest of the
    /// closed set: a binding can be switched to one of them and to nothing else, so a port
    /// cannot quietly become a contract the hardware was never built for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<String>,
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
    /// The adapter's own device is the only thing it talks to.
    ///
    /// A radio protocol has one process holding several coordinators, and the adapter's device
    /// is the software rather than any of them — so nothing starts until a coordinator is
    /// adopted under it. A service has one server and no children: the device somebody
    /// configured *is* the thing, and waiting for a child that will never exist means an
    /// adapter that never runs.
    #[serde(default)]
    pub single: bool,
    /// A container this adapter cannot work without. See [`DockerDecl`].
    #[serde(default)]
    pub docker: Option<DockerDecl>,
    /// The port of the adapter's own web interface, which the controller proxies into the driver's
    /// screen.
    ///
    /// Some adapters front software that already has a good screen of its own — Music Assistant's
    /// providers, its library, its scan of a folder. Re-drawing that in the configurator would
    /// mean re-drawing somebody else's application badly and again after each of their releases,
    /// and pointing an iframe straight at it only works from a browser that can reach the service
    /// directly. The controller can, so the controller carries it.
    ///
    /// A port and not the name of a property holding one. It was the latter for a day, on the
    /// reasoning that a service somebody moved should be an edit rather than a build — which is
    /// true of a service somebody else runs and false of one the controller starts itself. This is
    /// the port in [`DockerDecl::ports`], written once more; a driver that made it configurable
    /// would be asking an installer to keep two numbers in step for no reason.
    ///
    /// The host is the device's `Address` when it has one, and this controller when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_port: Option<u16>,
}

/// A container the controller runs for an adapter.
///
/// Most adapters are a script and a radio and need nothing of the sort. Some are a front end to
/// software that is only distributed as an image — Music Assistant is a Python application with
/// ffmpeg, a database and two dozen provider integrations behind it, and packaging that into a
/// `.junodrv` would mean maintaining somebody else's build.
///
/// So the driver names an image, and the controller runs it *if it can*. Docker is not a
/// requirement of Juno and never will be: a controller without it refuses this driver at install
/// with a sentence saying why, rather than accepting it and being quietly broken. See
/// `crate::driver::docker` in the controller for that half.
///
/// The container is the controller's, not the operator's: it is named after the driver, its
/// volumes are named after the driver, and it is removed when the last device using that driver
/// leaves the project. An operator who wants to run Music Assistant themselves can — and then
/// this declaration is the wrong way to reach it, which is what the address property is for.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DockerDecl {
    /// Fully qualified, with an exact version. An unpinned tag is a lint finding rather than a
    /// refusal — it is legal and it is a mistake: a house that changes what it is running
    /// because somebody else pushed a tag has broken on its own, on a restart nobody asked for.
    pub image: String,
    /// `host:container`, as `docker run -p` takes them. Published rather than host-networked
    /// because a host network is a Linux-only trick and the ports an adapter needs are known.
    #[serde(default)]
    pub ports: Vec<String>,
    /// `name:/path`. `name` is a *named volume*, which the controller prefixes with the driver
    /// id — never a path on the host. A driver that could name a host path could name any of
    /// them.
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
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

/// A screen the driver ships, and where the configurator should offer it.
///
/// A driver's own page arrives as one self-contained file — see `ui/index.html` — and a driver
/// with more than one thing to show used to draw its own tab strip inside it. Rendered where it
/// actually appears, that is two rows of tabs an inch apart with nothing saying which owns what:
/// the configurator's device pane already has a strip across the top, and the frame sits under
/// it.
///
/// So the driver declares its panes here and the configurator puts them in the strip it already
/// has. Declared rather than announced by the page at load, because the strip has to be right
/// before a four-hundred-kilobyte page has been fetched — tabs that appear a moment after the
/// pane opens are tabs somebody has already clicked past.
///
/// `on` is the same scoping actions use, and it is the whole reason this is a list: a Zigbee
/// adapter and its radios are one driver, and "every network in the house" and "the devices on
/// this one" are not the same screen.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TabDecl {
    /// Handed back to the page to say which pane to show. The driver's own vocabulary.
    pub id: String,
    /// What the tab says.
    pub title: String,
    /// Which of this driver's devices offers it. See [`ActionOn`].
    #[serde(default)]
    pub on: ActionOn,
    /// What the configurator should draw, when the driver has not shipped a page.
    ///
    /// Empty means "render `ui/index.html` on this pane" — the escape hatch, for a driver whose
    /// screen is genuinely a program: a mesh map and a table of three thousand converters is not
    /// something to express in a manifest, and pretending otherwise produces a schema that grows
    /// a field per driver until it is a worse programming language.
    ///
    /// Everything else should be here. A driver that describes its screen gets core's own
    /// components drawing it — the same cards, the same grid, the same palette — which means it
    /// matches the app for nothing, keeps matching when the app moves, and costs the driver
    /// author no HTML, no polling, and no way to get any of it wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block: Vec<BlockDecl>,
}

/// One thing on a declared pane.
///
/// Four kinds, chosen because they are what every driver page written here has turned out to be:
/// a handful of readings, a list of something, the buttons, and the sentence explaining what the
/// buttons do. A fifth is a decision to take when a driver needs one, not before.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockDecl {
    /// A row of readings — the shape of the summary at the top of every device pane.
    Stats {
        #[serde(default)]
        title: String,
        field: Vec<FieldDecl>,
    },
    /// A list of something the driver holds. `from` must resolve to an array.
    Table {
        #[serde(default)]
        title: String,
        from: String,
        column: Vec<ColumnDecl>,
    },
    /// Buttons for actions this driver declares. Named rather than "all of them", because the
    /// order buttons appear in is a judgement about which one somebody reaches for.
    Actions {
        #[serde(default)]
        title: String,
        #[serde(default)]
        action: Vec<String>,
    },
    /// What the pane is for, in the driver's own words. The paragraph under the buttons that
    /// says what happens when you press one.
    Text {
        #[serde(default)]
        title: String,
        body: String,
    },
}

/// Where a value comes from, and what to call it.
///
/// Three sources, and they are the three things core already holds about a device: what it last
/// reported, what somebody configured, and whatever its adapter published about itself.
///
///   `state.<key>`       a key on this device's own binding — `on`, `level`, `temperature`
///   `property.<name>`   a property as resolved for it, inherited from a bridge included
///   `detail.<path>`     a dotted path into the adapter's published detail, for a coordinator
///
/// A path that resolves to nothing draws an em dash rather than an error: a radio that has not
/// reported yet and a driver with a typo look the same from here, and neither is worth a red box
/// in the middle of a device pane. The typo is caught at install instead — see `validate`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDecl {
    pub from: String,
    pub label: String,
    /// Appended to the value. `°C`, `%`, `dBm`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    /// How to draw it: `text` (the default), `since` for a timestamp to render as an age,
    /// `bool` for a yes/no, `bytes` for a size.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
}

/// One column of a declared table. `path` is relative to each row of the array `from` named.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnDecl {
    pub path: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
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
    /// An adapter's own device — the process, not a radio under it. Everything true of the
    /// stack rather than of one network: how many radios there are, what the driver can
    /// describe, whether the child process is up.
    ///
    /// Distinct from [`Self::Own`], which covers this device *and* the radios, because a
    /// Zigbee adapter and a Zigbee coordinator are the same driver id and nothing else tells
    /// them apart. Offering "open the network" on the adapter is offering a button with no
    /// radio behind it.
    Adapter,
    /// One radio under an adapter — a Zigbee coordinator, a Z-Wave controller. The device a
    /// mesh actually hangs off.
    Coordinator,
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
            "adapter" => ActionOn::Adapter,
            "coordinator" | "radio" => ActionOn::Coordinator,
            "node" => ActionOn::Node,
            _ => ActionOn::Proxy(s),
        })
    }
}

impl Serialize for ActionOn {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            ActionOn::Own => "own",
            ActionOn::Adapter => "adapter",
            ActionOn::Coordinator => "coordinator",
            ActionOn::Node => "node",
            ActionOn::Proxy(p) => p,
        })
    }
}

/// Whether a declared field or table names a source core can resolve.
///
/// The three prefixes are checked, not the rest: `state.level` on a device with no level is a
/// dash, and so it should be — a battery sensor that has not woken yet has no reading and that
/// is not a manifest error. What is an error is `sate.level`, which would be a dash for ever.
fn check_source(from: &str) -> Result<(), String> {
    let (source, rest) = from.split_once('.').unwrap_or((from, ""));
    match source {
        "state" | "property" | "detail" if !rest.is_empty() => Ok(()),
        "state" | "property" | "detail" => Err(format!("`{from}` names no key")),
        other => Err(format!(
            "`{from}` reads from `{other}`, which is not one of state, property or detail"
        )),
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
    /// The driver cannot work without it. Read only for a patched kind — a dialled one is
    /// required by definition, since there is nothing for an installer to leave undone.
    #[serde(default = "default_true")]
    pub required: bool,
    /// Which of this driver's own proxies this connection serves, when only one of them does.
    pub proxy: Option<LocalId>,

    // ---- dialled kinds only. Absent on a patched one, and refused there by `validate`.
    /// The port core dials, for the connections whose socket core owns — the ones a driver
    /// speaks over `HostCall::Tx` or `HostCall::Publish`.
    ///
    /// Leave it unset for a driver that builds its own URLs through `HostCall::Http`: nothing
    /// reads it there, and a second copy of the port beside the one in the code is a copy that
    /// will disagree. Also unset when the port is announced rather than fixed — a Companion
    /// link and a HAP accessory pick one at boot and put it in their SRV record, which arrives
    /// as the device's own `Port` property and wins over this anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Hold the socket open. For a link whose point is the session — a pairing that costs two
    /// round trips, an event subscription that only delivers while the socket is up.
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
    /// at the first `0x0A` inside its length prefix and has every non-UTF-8 byte replaced, which
    /// for a ciphertext is most of them.
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Ask the device a question on connect and check what comes back — see [`Probe`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// Only meaningful on one of the driver's connections.
    ///
    /// A driver reached two ways often wants different answers for each: a panel over its
    /// security protocol needs a token off a settings page, and the same panel over its
    /// automation protocol needs a certificate exchange it does itself. Shown together, an
    /// installer gets five fields of which two are dead, and no way to tell which — which is
    /// the same bug as a slider that does nothing.
    ///
    /// Unset means it applies whatever the device is reached over, which is true of an address
    /// and of nearly everything else. Ignored by a driver with one connection, where there is
    /// nothing to be irrelevant to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<LocalId>,
}

/// The signature of a device on a Zigbee mesh: what its descriptor says about itself.
///
/// Deliberately the same three fields a `zigbee/<id>.json` fingerprint already carries, so a
/// package says it once and the index can carry it without a second vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZigbeeMatch {
    /// The application profile id. `49297` is the Control4/RTI one; `260` is Home Automation.
    pub profile: u16,
    pub endpoint: u8,
    /// Clusters the endpoint must offer. A node offering more still matches — a descriptor is
    /// a list of what a device *can* do, and requiring an exact set would refuse every device
    /// that gained a cluster in a firmware update.
    #[serde(default)]
    pub in_clusters: Vec<u16>,
}

impl ZigbeeMatch {
    /// Whether a node's descriptor satisfies this rule.
    pub fn matches(&self, profile: u16, endpoint: u8, clusters: &[u16]) -> bool {
        self.profile == profile
            && self.endpoint == endpoint
            && self.in_clusters.iter().all(|c| clusters.contains(c))
    }
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
    /// What an unknown node on the mesh has to look like for this driver to be the answer —
    /// see [`ZigbeeMatch`].
    ///
    /// Beside the other four rather than anywhere else, because it is the same question asked
    /// of a different network: a coordinator reports a node nobody recognises, and the
    /// controller matches it against the whole catalog and offers the package. Without it a
    /// mesh device can only be adopted once somebody has already guessed which driver to
    /// install, which is the one thing discovery exists to remove.
    #[serde(default)]
    pub zigbee: Vec<ZigbeeMatch>,
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
    pub property: Vec<PropertyDecl>,
    #[serde(default)]
    pub discovery: Discovery,
    /// Driver-specific actions. See [`ActionDecl`] for why these are not proxy commands.
    #[serde(default)]
    pub action: Vec<ActionDecl>,
    /// Panes the driver's own page offers, for the configurator's tab strip. See [`TabDecl`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tab: Vec<TabDecl>,
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

    /// Whether this driver says one of its own bindings may present `want`.
    ///
    /// The gate on switching a combo port. A binding whose type is not in the driver's own
    /// closed set is a port claiming a contract its hardware was never built for, which
    /// something downstream will then route a room through.
    pub fn binding_may_be(&self, local: LocalId, want: &str) -> bool {
        self.proxy
            .iter()
            .find(|p| p.id == local)
            .is_some_and(|p| p.ty == want || p.alternates.iter().any(|a| a == want))
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

    /// The properties that apply to a device reached over `connection`.
    ///
    /// Everything untagged, plus what this connection claims. A device that has not said which
    /// way in it uses gets all of them — which is the honest answer while nobody knows, and is
    /// every device of every single-connection driver.
    pub fn properties_for(&self, connection: Option<LocalId>) -> Vec<&PropertyDecl> {
        self.property
            .iter()
            .filter(|p| match (p.connection, connection) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(mine), Some(theirs)) => mine == theirs,
            })
            .collect()
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

        // A declared pane is checked here for the same reason an action is: nothing else ever
        // looks at it. A `from` naming a source that does not exist draws an em dash for ever,
        // which is indistinguishable from a radio that has not reported — so the typo has to be
        // caught at install, where somebody is looking at the driver they just wrote.
        let mut tab_ids = BTreeSet::new();
        for t in &self.tab {
            if !tab_ids.insert(t.id.as_str()) {
                errs.push(format!("duplicate tab `{}`", t.id));
            }
            if t.id.is_empty() || t.title.is_empty() {
                errs.push(format!("tab `{}` needs an id and a title", t.id));
            }
            for block in &t.block {
                match block {
                    BlockDecl::Stats { field, .. } => {
                        for f in field {
                            if let Err(e) = check_source(&f.from) {
                                errs.push(format!("tab `{}`: {e}", t.id));
                            }
                        }
                    }
                    BlockDecl::Table { from, column, .. } => {
                        if let Err(e) = check_source(from) {
                            errs.push(format!("tab `{}`: {e}", t.id));
                        }
                        if column.is_empty() {
                            errs.push(format!("tab `{}`: a table with no columns", t.id));
                        }
                    }
                    // Named rather than "every action", so this catches the rename that would
                    // otherwise leave a button drawn against nothing.
                    BlockDecl::Actions { action, .. } => {
                        for name in action {
                            if !self.action.iter().any(|a| &a.name == name) {
                                errs.push(format!(
                                    "tab `{}` offers `{name}`, which this driver does not declare",
                                    t.id
                                ));
                            }
                        }
                    }
                    BlockDecl::Text { body, .. } => {
                        if body.trim().is_empty() {
                            errs.push(format!("tab `{}`: a text block with nothing in it", t.id));
                        }
                    }
                }
            }
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
        for p in &self.proxy {
            for alt in &p.alternates {
                if registry.get(alt).is_none() {
                    errs.push(format!(
                        "proxy {}: `{alt}` is not a proxy in this core",
                        p.id
                    ));
                }
                if *alt == p.ty {
                    errs.push(format!(
                        "proxy {}: `{alt}` is already what it is — an alternate is what else \
                         the same port can be",
                        p.id
                    ));
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
            if c.name.trim().is_empty() {
                errs.push(format!(
                    "control {}: needs a name — a driver with two connections asks somebody \
                     which, and `1` and `2` is not a question anyone can answer",
                    c.id
                ));
            }
            if let Some(want) = c.kind.provider_proxy()
                && registry.get(want).is_none()
            {
                errs.push(format!("control {}: no `{want}` proxy in this core", c.id));
            }
            if let Some(p) = c.proxy
                && !self.proxy.iter().any(|d| d.id == p)
            {
                errs.push(format!("control {}: proxy {p} is not declared", c.id));
            }
            // A field that means nothing for this kind is a mistake worth catching while
            // somebody is looking at the manifest. Silently ignoring it is how a driver ships
            // with a port nothing dials and an author who believes it is dialled.
            if !c.kind.is_dialled() {
                for (field, set) in [
                    ("port", c.port.is_some()),
                    ("tls", c.tls),
                    ("keepalive", c.keepalive),
                    ("binary", c.binary),
                    ("username", c.username.is_some()),
                    ("password", c.password.is_some()),
                    ("probe", c.probe.is_some()),
                ] {
                    if set {
                        errs.push(format!(
                            "control {}: `{field}` means nothing on a `{:?}` connection — \
                             nothing opens a socket for it",
                            c.id, c.kind
                        ));
                    }
                }
            }
        }
        for p in &self.property {
            if let Some(c) = p.connection
                && !self.control.iter().any(|decl| decl.id == c)
            {
                errs.push(format!(
                    "property `{}` belongs to connection {c}, which this driver does not declare",
                    p.name
                ));
            }
        }
        // Two connections of the same kind are two of the same thing, and nothing downstream
        // can tell them apart when it matters — which port to dial, which one a device is set
        // up on. Two *different* kinds is the whole point of the table.
        let mut kinds = BTreeSet::new();
        for c in &self.control {
            if c.kind.is_dialled() && !kinds.insert(c.kind) {
                errs.push(format!(
                    "two `{:?}` connections — a driver reached two ways has to be reached two \
                     different ways",
                    c.kind
                ));
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

            [[control]]
            id = 1
            kind = "network"
            name = "API"
            port = 7421

            [control.probe]
            send = "{\"op\":\"hello\",\"token\":\"\"}\n"
            expect = "\"op\":\"denied\""
            "#,
        )
        .expect("manifest with a probe should parse");

        let probe = m.control[0].probe.as_ref().expect("probe should be there");
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

    /// A connection without one is the normal case, and must stay the normal case: a missing
    /// `[control.probe]` is what stops a controller sweeping the network for every driver.
    #[test]
    fn a_connection_without_a_probe_sweeps_nothing() {
        let m = manifest_with("roku.tv").unwrap();
        assert!(m.control.iter().all(|c| c.probe.is_none()));
    }

    /// Fixed MQTT credentials belong on the manifest — the whole product line shares one
    /// broker password — and have to survive a round trip through TOML same as `port` does.
    #[test]
    fn mqtt_credentials_on_a_connection_parse() {
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
            [[control]]
            id = 1
            kind = "mqtt"
            name = "Broker"
            port = 36669
            tls = true
            username = "hisenseservice"
            password = "multimqttservice"
            "#,
        )
        .expect("manifest with mqtt credentials should parse");
        let c = &m.control[0];
        assert_eq!(c.username.as_deref(), Some("hisenseservice"));
        assert_eq!(c.password.as_deref(), Some("multimqttservice"));

        // Absent for every connection that has never needed one — a Roku, a Denon.
        let plain = manifest_with("roku.tv").unwrap();
        assert!(
            plain
                .control
                .iter()
                .all(|c| c.username.is_none() && c.password.is_none())
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
