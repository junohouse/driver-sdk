//! The driver host: what a driver is, and what it can ask core to do.
//!
//! # One module, many devices
//!
//! A driver is loaded **once per driver, not once per device**. Five Roku TVs share one
//! loaded module; what makes them five different TVs is five [`Instance`]s — properties,
//! per-device scratch state, and their own control links and bindings. The module is `&self`
//! throughout, so it holds no device state and cannot accidentally leak one TV's state into
//! another.
//!
//! That matters concretely on the target hardware: for WASM the expensive part is compiling
//! the module, and it happens once; for Python it is one subprocess for all five, not five.

use crate::LocalId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type DeviceId = u32;
pub type Args = BTreeMap<String, Value>;

/// Which way signal travels through a connection, from the device's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Consumer,
    Provider,
}

/// One signal endpoint: an HDMI input on a television, the output of a disc player.
///
/// Here rather than in [`crate::manifest`], where it is still re-exported from, because a driver
/// builds these to answer [`HostCall::Connections`] and `manifest` is behind the `contracts`
/// feature that a driver does not compile.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionDecl {
    /// Driver-local and **stable across restarts** — a project remembers what an installer
    /// wired by this number, so renumbering moves somebody's cabling.
    pub id: LocalId,
    pub proxy: LocalId,
    pub dir: Direction,
    /// `HDMI`, `Optical`, `Analog`. What the pathfinder costs and matches a signal against.
    pub class: String,
    pub name: String,
}

/// Something a driver asks core to do. Drivers return these rather than calling back into
/// core, which keeps them synchronous, pure, and trivially testable — and means core decides
/// what actually happens, including refusing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum HostCall {
    /// Command a bound control connection. Core resolves `control` to whatever provider
    /// binding the installer wired it to — a relay, an IR port, a serial port.
    Invoke {
        control: LocalId,
        cmd: String,
        args: Args,
    },
    /// Raw bytes at a control connection. `control: 0` means the driver's own network
    /// transport — the one case core owns directly, because there is no physical port to
    /// bind and no second driver in the path.
    Tx {
        control: LocalId,
        data: Vec<u8>,
    },
    /// An HTTP request. Core owns the client so drivers cannot each ship their own, and so
    /// timeouts, retries, and TLS are enforced in one place.
    Http(HttpRequest),
    /// Publish on this device's MQTT connection. Requires `[[transport]] kind = "mqtt"`.
    ///
    /// Core is the *client* here, never a broker: every MQTT device already is one or has one
    /// beside it, and the topology is one connection per device with nothing shared. So this is
    /// the same arrangement as [`Self::Tx`] — core owns the socket, the TLS and the reconnect —
    /// with a topic instead of a stream position.
    Publish {
        topic: String,
        payload: String,
    },
    /// Put this on the device's own live control channel.
    ///
    /// For a device with a screen somebody is looking at *right now* — a navigator being driven
    /// by a remote in the same room. Deliberately not a notification and deliberately not state:
    /// a press is not a fact about the house that can be read back later, it is an instruction
    /// that matters for the half second after it happens.
    ///
    /// That difference is the whole reason this exists rather than reusing the revision stream
    /// every other screen watches. That stream coalesces — it says "something changed, read the
    /// house again" — which is exactly right for state and wrong for input: two `up` presses in
    /// one tick are one revision, and a page that re-read state would move the cursor once.
    /// Frames here are delivered individually, in order, and are dropped rather than queued for
    /// a screen nobody is watching.
    Control {
        /// Which of the device's channels this belongs on.
        ///
        /// One device can have more than one thing listening to it, and they are not
        /// interchangeable: a navigator is both a page being navigated and a box wired into a
        /// television, and the browser cannot act on a CEC message any more than the CEC daemon
        /// can move a cursor. Naming the channel keeps each subscriber reading only what it can
        /// act on, instead of every listener filtering a shared firehose by guessing.
        channel: String,
        payload: serde_json::Value,
    },
    /// The signal connections this device actually has, as it currently is.
    ///
    /// `[[connection]]` in a manifest is written before anybody has plugged anything in, so for
    /// a whole product line it can only be a guess — and it is wrong in both directions at once:
    /// a manifest declaring four HDMI ports gives a three-port set a phantom fourth that the
    /// pathfinder will happily route a room through, and gives a six-port set two inputs nobody
    /// can select. Every television that speaks to a controller at all can be *asked*, so this
    /// is the driver answering with what it found.
    ///
    /// A **snapshot**, never a delta, and replacing the manifest's list outright when present —
    /// the same bargain [`Self::Present`] makes and for the same reason: after a reconnect a
    /// driver says what it has now and core reconciles, with no resync path to get wrong. An
    /// empty list is therefore meaningful (this device has no signal connections) and is
    /// distinct from never having sent one (use the manifest's).
    ///
    /// Ids must be **stable across restarts and firmware updates** — a project remembers what an
    /// installer wired by this number, so a driver that renumbers its inputs moves somebody's
    /// cabling. Prefer deriving them from the device's own identifiers rather than from the
    /// order a list happened to arrive in.
    Connections {
        connections: Vec<ConnectionDecl>,
    },
    /// Import or refresh provider-owned scenes discovered by an already-adopted controller.
    ///
    /// This is deliberately a one-way snapshot into Core. It cannot ask the provider to create,
    /// update, or delete anything; Core stores each entry as a borrowed read-only handle and may
    /// only recall it through [`crate::SceneOperation::Recall`]. `steps` identify installed child
    /// devices by stable, non-secret properties because a driver never knows Core's numeric device
    /// ids. Juno-owned provider scenes must not be included.
    BorrowedScenes {
        scenes: Vec<BorrowedSceneSnapshot>,
    },
    /// A Wake-on-LAN magic packet, broadcast on core's behalf.
    ///
    /// The one case a driver dials nobody: the device is asleep and there is no socket to hold
    /// open or open, only a frame to put on the wire. Core owns the UDP broadcast for the same
    /// reason it owns every other socket — a driver has no business doing its own network I/O —
    /// and there is nothing to hold open afterward, unlike [`Self::Publish`] or [`Self::Tx`].
    Wol {
        mac: String,
    },
    /// Ask for a topic on this device's MQTT connection.
    ///
    /// Remembered by core and asked for again after a reconnect, because a subscription is state
    /// held by the broker and the broker is the thing that just restarted. Subscribing twice to
    /// the same topic is harmless and does nothing.
    ///
    /// A call rather than a manifest field because the topics that matter are rarely constant: a
    /// panel answers on a topic named after the client id it issued during pairing, which nobody
    /// knows when the manifest is written.
    Subscribe {
        topic: String,
    },
    /// Emit a proxy notification. Validated against the declared capabilities before it
    /// reaches anything else.
    Notify {
        proxy: LocalId,
        name: String,
        args: Args,
    },
    /// Set a state value directly. For state that no single notification parameter implies —
    /// a heat setpoint arriving as `setpoint_changed{which: "heat", celsius: 21}`, where the
    /// key to write depends on a sibling field.
    SetState {
        proxy: LocalId,
        key: String,
        value: Value,
    },
    /// Everything this device has behind it, as it currently is.
    ///
    /// The answer to a driver that cannot know its own shape until it has asked. A manifest's
    /// `[[proxy]]` blocks are written before anybody has plugged anything in, which is right for
    /// a television and impossible for a hub: an alarm panel has as many zones as somebody
    /// programmed into it, and declaring 128 `sensor` proxies so the largest possible panel fits
    /// would give every real house a hundred empty bindings.
    ///
    /// A **snapshot**, never a delta — the same bargain [`crate::adapter::Up::Present`] makes, and
    /// it goes through the same code in core. A delta protocol needs a resync path, and a resync
    /// path is a snapshot protocol with extra steps: after a reconnect a driver simply says what
    /// it has now, and core reconciles. Nodes that stop appearing stop being offered; ones already
    /// adopted keep their bindings and their history.
    ///
    /// Gated by the manifest's `[children] proxies`. A driver may only present kinds it declared,
    /// so a security panel cannot grow a `lock` the day its vendor's firmware learns a new word —
    /// see [`crate::manifest::ChildrenDecl`].
    Present {
        nodes: Vec<Node>,
    },
    /// Aim these calls at one of this device's nodes rather than at the device itself.
    ///
    /// [`Self::Notify`] resolves its proxy against the device that returned it, which is exactly
    /// right until a driver holds one connection on behalf of forty things behind it. The panel
    /// hears that zone 7 opened; the binding that has to move belongs to zone 7's device, and
    /// before this there was no way to say so from in-process code.
    ///
    /// A node nobody has adopted is not an error — it is in the offers list and its reports are
    /// not state until somebody claims it — so calls for one are dropped quietly, the same as
    /// [`crate::adapter::Up::Push`] for an unadopted node.
    ForNode {
        node: String,
        calls: Vec<HostCall>,
    },
    /// The hardware behind this device is gone, and the driver knows it first-hand.
    ///
    /// For a hub that reports its own removals — a Hue bridge pushes `delete` on its event stream
    /// when somebody unpairs a bulb in the vendor's app — this is the other half of
    /// `device_added`. Core forgets the device outright, which is what somebody who removed it at
    /// the hub meant; anything less leaves a tile that is permanently offline and a rule that
    /// silently never fires again.
    ///
    /// **Only for hardware a driver is certain has been removed at its source.** It is not
    /// "unreachable": a bulb at the far end of a mesh, a bridge halfway through a reboot and a
    /// house whose Wi-Fi dropped are all still there, and deleting them because a poll timed out
    /// would delete somebody's house every time their router restarts. Offline is
    /// `online_changed`, which is state and comes back on its own. This is a one-way door — see
    /// [`Runtime::forget_device`], which takes the device's rules with it.
    ///
    /// `reason` is shown to whoever finds out afterwards, so it should say where the removal was
    /// observed rather than restate that something was deleted.
    Gone {
        reason: String,
    },
    Log {
        level: String,
        msg: String,
    },
}

/// One device a driver is offering, already mapped to Juno's semantics.
///
/// The mapping from a Zigbee cluster, a Z-Wave command class or an alarm panel's zone type to a
/// proxy contract lives in the **driver**, never here. Core has no idea what cluster `0x0006` is
/// and must never learn: that knowledge changes with every firmware and every quirk, and putting
/// it in core would mean shipping a controller release to support a new bulb.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Node {
    /// Stable and driver-assigned. An IEEE address, a Z-Wave node id, a panel's zone number —
    /// something that survives the device being renamed, re-roomed, or the driver restarting.
    pub node: String,
    pub name: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub model: String,
    /// Which proxy contract this node is: `light`, `switch`, `sensor`, `lock`.
    pub kind: String,
    /// What *this particular device* can do, resolved against the contract named by `kind`.
    ///
    /// Per node, and that is the whole point rather than a refinement. A mesh is not a fleet of
    /// one product: a plain white bulb and an extended-color bulb are both `light`, and if they
    /// resolve to the same contract then `set_cct` appears in the UI, in the automation editor
    /// and in the assistant's tool surface for a bulb that cannot do it — and `validate_call`
    /// waves it through to a driver that fails in silence. Somebody then spends an evening
    /// deciding their new bulb is faulty.
    ///
    /// The driver is the only thing that can fill this in honestly, because the answer lives in
    /// `zigbee-herdsman-converters` — a decade of per-model capability data that is exactly the
    /// driver work nobody should repeat. Core does not have it and should never learn it: that
    /// database changes weekly, and baking it in would mean a controller release per new bulb.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    /// Whether the driver can currently reach it. A battery sensor that has not reported is
    /// not a fault, so this is shown rather than acted on.
    #[serde(default = "yes")]
    pub online: bool,
    /// Where the far side says this device lives. Empty when it has no idea, which is the
    /// normal case for a radio.
    ///
    /// A **suggestion**, and the distinction is the whole reason this is safe. Rooms belong to
    /// the project, and nothing here creates one: an offered node carries the name through to the
    /// moment an installer adopts it, and core matches or creates only then, with the list on
    /// screen. A driver still cannot make a room while nobody is looking, rename one, or delete
    /// one.
    ///
    /// It exists because some systems genuinely know. A Zigbee mesh does not, but a Control4
    /// project does — every device in it is already filed under a room somebody named — and
    /// throwing that away would mean hand-placing several hundred devices to import a house
    /// that had already been commissioned once.
    #[serde(default)]
    pub room: String,
    /// Which of the driver's per-device settings *this* node actually has.
    ///
    /// `capabilities` says what the device can be commanded to do, resolved against its contract.
    /// This is the other half: the knobs that are not commands at all — an occupancy hold, a
    /// sensitivity — which a driver exposes as `[[action]]` and which exist on some devices of a
    /// class and not others. An SNZB-06P and a door contact are both `sensor`; only one has a
    /// hold time.
    ///
    /// Same reasoning as `capabilities`, and the same reason it cannot live in core: the answer
    /// is in `zigbee-herdsman-converters`, changes weekly, and is per model. An action naming
    /// one of these in [`crate::manifest::ActionDecl::needs_one_of`] appears only on the nodes
    /// that reported it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<String>,
}

/// Hand-written rather than derived, because `online` defaults to *true*.
///
/// A node is present unless it says otherwise — the same rule as the serde default above, and
/// deriving `Default` would quietly give every node built this way `online: false`.
impl Default for Node {
    fn default() -> Node {
        Node {
            node: String::new(),
            name: String::new(),
            manufacturer: String::new(),
            model: String::new(),
            kind: String::new(),
            capabilities: BTreeMap::new(),
            online: true,
            room: String::new(),
            settings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        HttpRequest {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn json(mut self, body: impl Into<String>) -> Self {
        self.headers
            .push(("content-type".into(), "application/json".into()));
        self.body = Some(body.into());
        self
    }

    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

impl HostCall {
    pub fn warn(msg: impl Into<String>) -> Self {
        HostCall::Log {
            level: "warn".into(),
            msg: msg.into(),
        }
    }

    pub fn notify(proxy: LocalId, name: &str, args: Args) -> Self {
        HostCall::Notify {
            proxy,
            name: name.into(),
            args,
        }
    }

    /// See [`HostCall::Gone`] — for hardware removed at its source, never for hardware that has
    /// merely stopped answering.
    pub fn gone(reason: impl Into<String>) -> Self {
        HostCall::Gone {
            reason: reason.into(),
        }
    }
}

/// Everything that makes one device different from another running the same driver.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Instance {
    pub device: DeviceId,
    /// Installer-set values from the manifest's `property` blocks.
    pub properties: BTreeMap<String, Value>,
    /// The driver's own per-device memory. Survives restart when persisted.
    pub scratch: BTreeMap<String, Value>,
}

/// One physical member of a Juno logical group.
///
/// Core supplies the member's own driver instance and proxy state so the bridge driver can
/// prove that a vendor-side group still has exactly the same members before using it. Secrets
/// stay where they already live: inherited bridge properties are not copied into the member.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupMember {
    pub device: DeviceId,
    pub proxy: LocalId,
    pub instance: Instance,
    #[serde(default)]
    pub state: Args,
}

/// What Core is asking a native-group provider to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum GroupOperation {
    /// Describe the current link and vendor-side groups that are safe to select.
    Status,
    /// Link the logical group to an existing vendor-side resource without taking ownership.
    Link { resource: String },
    /// Create a new vendor-side resource owned by Juno.
    Create,
    /// Bring a Juno-owned resource's name and membership back in sync.
    Synchronize,
    /// Stop using the vendor-side resource. This never implies deleting it.
    Detach,
    /// Execute one proxy command through the linked vendor-side group.
    Command {
        command: String,
        #[serde(default)]
        args: Args,
    },
}

/// A native grouped-control request. The provider instance passed to [`DriverModule::on_group`]
/// is the bridge/controller that owns the connection; this structure describes the logical group
/// and its physical children.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRequest {
    pub group: DeviceId,
    pub name: String,
    #[serde(default)]
    pub state: Args,
    pub members: Vec<GroupMember>,
    pub operation: GroupOperation,
}

/// Whether Core should consider a native group request complete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupDisposition {
    /// Use Core's existing per-member behavior.
    #[default]
    Unsupported,
    /// The request was accepted and any returned calls should be executed.
    Handled,
    /// Work was started but needs an asynchronous response before status changes.
    Queued,
    /// The provider supports groups but refused this request for the reported reason.
    Refused,
}

/// Calls that update a physical member after one bridge-level request.
///
/// Core deliberately permits only state/notification/log calls in this list. Network I/O belongs
/// to the provider's `calls`, preventing a group driver from using sibling devices as an escape
/// hatch for extra connections.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupMemberCalls {
    pub device: DeviceId,
    #[serde(default)]
    pub calls: Vec<HostCall>,
    /// Updated per-device driver memory after the provider has applied the grouped command.
    /// Omitted when the provider did not change it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<BTreeMap<String, Value>>,
}

/// Result of native group handling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupResponse {
    #[serde(default)]
    pub disposition: GroupDisposition,
    /// Driver-defined, non-secret status for API/UI presentation.
    #[serde(default)]
    pub status: Value,
    /// Calls executed on the provider bridge/controller.
    #[serde(default)]
    pub calls: Vec<HostCall>,
    /// Optimiztic state updates for physical group members.
    #[serde(default)]
    pub members: Vec<GroupMemberCalls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One command stored for a member of a controller-native scene.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneAction {
    pub command: String,
    #[serde(default)]
    pub args: Args,
}

/// One physical member of a controller-native scene.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneMember {
    pub device: DeviceId,
    pub proxy: LocalId,
    pub instance: Instance,
    #[serde(default)]
    pub actions: Vec<SceneAction>,
}

/// A color in a controller-run dynamic palette.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScenePaletteColor {
    /// CIE xy chromaticity, the vendor-neutral color coordinates used by Hue's v2 scene API.
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>,
}

/// A native effect requested for one light in a scene.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneEffect {
    pub device: DeviceId,
    pub effect: String,
}

/// Optional behavior that must be executed by the controller/lights, never by a Core update loop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneAnimation {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub palette: Vec<ScenePaletteColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(default)]
    pub auto_dynamic: bool,
    #[serde(default)]
    pub effects: Vec<SceneEffect>,
}

/// Who is allowed to change the provider-side scene resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneOwnership {
    /// A scene created from Juno and recorded by the provider as Juno-owned.
    #[default]
    Juno,
    /// A scene imported from the provider. It may be recalled but never changed or detached
    /// provider-side by Juno.
    Borrowed,
}

/// How a provider-native scene should be recalled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneRecall {
    #[default]
    Static,
    Dynamic,
}

/// What Core is asking a native-scene provider to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SceneOperation {
    Status,
    /// Create the scene if it has not been published, otherwise synchronize it. The provider
    /// must refuse unless its local ownership record proves the existing resource is Juno-owned.
    Synchronize,
    /// Stop using a Juno-owned provider resource locally. This never implies deleting it.
    Detach,
    Recall {
        mode: SceneRecall,
    },
}

/// A native-scene request. `resource` is set for a borrowed import; Juno-created scenes rely on
/// the provider's bridge-scoped ownership record instead of accepting an arbitrary writable id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneRequest {
    pub scene: u32,
    pub name: String,
    pub ownership: SceneOwnership,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default)]
    pub members: Vec<SceneMember>,
    #[serde(default)]
    pub animation: SceneAnimation,
    pub operation: SceneOperation,
}

/// Result of native scene handling. Network calls belong to the controller; member calls are
/// restricted by Core to state, notification, and log updates just like native groups.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneResponse {
    #[serde(default)]
    pub disposition: GroupDisposition,
    #[serde(default)]
    pub status: Value,
    #[serde(default)]
    pub calls: Vec<HostCall>,
    #[serde(default)]
    pub members: Vec<GroupMemberCalls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl Instance {
    pub fn new(device: DeviceId) -> Self {
        Instance {
            device,
            ..Default::default()
        }
    }

    pub fn property(&self, name: &str) -> &Value {
        self.properties.get(name).unwrap_or(&Value::Null)
    }
}

/// Loaded driver code, shared by every device using it.
///
/// `&self` is deliberate: a module that wanted per-device mutable state would have to put it
/// in [`Instance`], which is exactly where it belongs.
pub trait DriverModule: Send + Sync {
    /// Run a driver-declared action. See the manifest's `[[action]]`.
    ///
    /// Defaulted to nothing on purpose: an action can only be invoked if the driver's own
    /// manifest declares it, so a driver built before actions existed is never asked to run
    /// one. That is what lets this land without an ABI bump — the manifest is the gate, not the
    /// trait.
    fn on_action(&self, _inst: &mut Instance, _action: &str, _args: &Args) -> Vec<HostCall> {
        Vec::new()
    }

    fn on_command(
        &self,
        inst: &mut Instance,
        proxy: LocalId,
        cmd: &str,
        args: &Args,
    ) -> Vec<HostCall>;

    /// Use a bridge/controller's vendor-native grouped control for a Juno logical group.
    ///
    /// Core calls this only when the provider's manifest explicitly declares `group_control`,
    /// and falls back to the normal per-member dispatch when the response is unsupported or
    /// refused. Drivers built before this contract therefore keep their existing behavior.
    fn on_group(&self, inst: &mut Instance, request: &GroupRequest) -> GroupResponse {
        let _ = (inst, request);
        GroupResponse::default()
    }

    /// Store, describe, or recall a scene on the bridge/controller.
    ///
    /// Core calls this only when the provider opts into `driver.scene_control`. Dynamic recalls
    /// are one native request; a provider must never implement them as a repeated REST loop.
    fn on_scene(&self, inst: &mut Instance, request: &SceneRequest) -> SceneResponse {
        let _ = (inst, request);
        SceneResponse::default()
    }

    /// A command for one of the nodes this device presented. See [`HostCall::Present`].
    ///
    /// `inst` is the **parent's** instance, not the node's, and that is deliberate rather than
    /// convenient: the address, the token and the open connection belong to the panel, and calls
    /// returned from here go out on the panel's transport. A node is an address within a
    /// connection somebody else owns, so anything else would mean forty devices each holding
    /// their own copy of one socket's credentials.
    ///
    /// `kind` is the node's proxy contract, so a driver that presents both locks and lights can
    /// switch on it without keeping its own table of which node is which.
    ///
    /// Defaulted, like [`Self::on_action`] and for the same reason: a driver is only ever sent one
    /// of these for a node it presented itself, so a driver built before this existed is never
    /// asked. The manifest is the gate, not the trait — no ABI bump.
    fn on_node_command(
        &self,
        inst: &mut Instance,
        node: &str,
        kind: &str,
        cmd: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        let _ = (inst, node, kind, cmd, args);
        Vec::new()
    }

    /// A notification arrived from a provider this device is bound to — the relay it drives
    /// changed state, bytes came back from its serial port.
    fn on_event(
        &self,
        inst: &mut Instance,
        control: LocalId,
        note: &str,
        args: &Args,
    ) -> Vec<HostCall> {
        let _ = (inst, control, note, args);
        Vec::new()
    }

    fn on_bind(&self, inst: &mut Instance) -> Vec<HostCall> {
        let _ = inst;
        Vec::new()
    }

    /// Features the manifest asked for that this build does not execute yet. Surfaced at
    /// install time so a driver author is not left guessing why nothing happens.
    fn unsupported(&self) -> Vec<String> {
        Vec::new()
    }

    /// Look for this driver's hardware. Called with an empty state to begin; core performs
    /// any [`SetupStep::Fetch`] and calls back with the response.
    ///
    /// The default is "this driver cannot find itself", which is honest for a cloud service
    /// or a device that announces nothing — those are set up through [`Self::setup`] instead.
    fn discover(&self, driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        let _ = (driver_id, state, input);
        (
            SetupStep::Done {
                devices: Vec::new(),
                rules: Vec::new(),
                scenes: Vec::new(),
            },
            Value::Null,
        )
    }

    /// Step through this driver's configuration. The driver decides what to ask and what the
    /// answers mean; core renders whatever it returns and performs the I/O.
    fn setup(&self, driver_id: &str, state: &Value, input: &Args) -> (SetupStep, Value) {
        self.discover(driver_id, state, input)
    }
}

// ---------------------------------------------------------------------------------------
// The plugin boundary
// ---------------------------------------------------------------------------------------

/// What core asks a separately-compiled driver to do.
///
/// JSON in, JSON out. Rust has no stable ABI, so passing trait objects across a `dylib`
/// boundary is undefined behavior waiting to happen; a serialized call is slower and
/// completely safe. At the rate a house generates commands, the difference is unmeasurable —
/// and this is the same shape the WASM runtime will use, so drivers do not change when it
/// arrives.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "fn", rename_all = "snake_case")]
pub enum Request {
    /// A driver-declared action. Gated by the manifest, so a driver built before actions
    /// existed is never sent one and needs no rebuild.
    OnAction {
        #[serde(default)]
        driver_id: String,
        action: String,
        args: Args,
        instance: Instance,
    },
    OnCommand {
        /// Which of the package's drivers this is for.
        #[serde(default)]
        driver_id: String,
        proxy: LocalId,
        cmd: String,
        args: Args,
        instance: Instance,
    },
    /// Native grouped control, gated by `driver.group_control` in the provider manifest.
    OnGroup {
        #[serde(default)]
        driver_id: String,
        request: GroupRequest,
        /// The provider bridge/controller instance.
        instance: Instance,
    },
    /// Native scene handling, gated by `driver.scene_control` in the provider manifest.
    OnScene {
        #[serde(default)]
        driver_id: String,
        request: SceneRequest,
        /// The provider bridge/controller instance.
        instance: Instance,
    },
    /// A command for a node, carrying the node's id and contract. Gated by the driver having
    /// presented that node, so one is never sent to a driver that does not implement it.
    OnNodeCommand {
        #[serde(default)]
        driver_id: String,
        node: String,
        kind: String,
        cmd: String,
        args: Args,
        /// The parent's instance — see [`DriverModule::on_node_command`].
        instance: Instance,
    },
    OnEvent {
        /// Which of the package's drivers this is for.
        #[serde(default)]
        driver_id: String,
        control: LocalId,
        note: String,
        args: Args,
        instance: Instance,
    },
    OnBind {
        /// Which of the package's drivers this is for.
        #[serde(default)]
        driver_id: String,
        instance: Instance,
    },
    /// Features the driver parsed but does not execute — reported at install time.
    Unsupported,
    /// Look for devices of this kind on the network. The driver knows how to recognize its
    /// own hardware; core does not, and must not.
    Discover {
        /// Which of the package's drivers this is for.
        #[serde(default)]
        driver_id: String,
        state: Value,
        input: Args,
    },
    /// Advance this driver's setup flow. Core carries the state and performs the I/O; the
    /// driver decides what to ask and what the answers mean.
    Setup {
        #[serde(default)]
        driver_id: String,
        state: Value,
        input: Args,
    },
}

/// A field in a driver's setup form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(default)]
    pub label: String,
    /// `string` · `password` · `int` · `bool` · `choice`
    #[serde(default = "text")]
    pub kind: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "yes")]
    pub required: bool,
}

fn text() -> String {
    "string".into()
}
fn yes() -> bool {
    true
}

/// One row of a [`SetupStep::Pick`] table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickRow {
    /// What comes back as the answer when this row is chosen.
    pub value: String,
    /// One per column, in order.
    pub cells: Vec<String>,
    /// Shown under the row — why this one might not be the right choice.
    #[serde(default)]
    pub note: String,
}

/// A device a driver found and is offering to set up.
///
/// `Default` is derived so a field added here later does not break every driver that builds one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Candidate {
    /// What to call it in the list — distinct enough to tell two of the same apart.
    pub label: String,
    #[serde(default)]
    pub kind: String,
    /// Which of the driver's manifests this is, when it ships more than one.
    #[serde(default)]
    pub driver_id: String,
    /// Properties to set on the device. Everything here came off the wire.
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    /// What *this one* can do, resolved against the contract its manifest names.
    ///
    /// The same field [`Node::capabilities`] is, for the same reason and with the same rule
    /// about who can answer honestly — a bridge's setup is the other way a driver learns what
    /// one device is, and it knows exactly as much as an inventory does. A Hue bridge says
    /// which of its lights has color and which is a white fitting on a lamp in the hall.
    ///
    /// The alternative is a manifest per shape: `bulb`, `bulb.color`, `bulb.dimmable`,
    /// `bulb.tunable`, `bulb.on_off` — five drivers for one product that differ in nothing but
    /// a capability line, listed in the catalog as five things to install, when Philips sells
    /// one thing called a light. Empty means the manifest's own declaration stands, which is
    /// right for a driver that ships one model.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    /// What the driver confirmed when it checked — proof it is really there.
    #[serde(default)]
    pub verified: String,
    /// Where the system being set up says this lives. Empty when it has no idea, which is the
    /// normal case.
    ///
    /// A **suggestion**, and the same one [`crate::adapter::Node::room`] makes, for the same
    /// reasons and with the same guarantees: rooms belong to the project, nothing here creates
    /// one behind anybody's back, and core matches or creates only at the moment an installer
    /// adopts — with the list on screen. A driver still cannot rename a room or delete one.
    ///
    /// It exists because a hub for a whole house is the case where hand-placing does not scale.
    /// A Hue bridge with forty bulbs on it already knows which room each one is in, because
    /// somebody sat down and filed them in the Hue app; without this, adopting that bridge means
    /// doing the same work a second time, from a list of forty devices called "Hue color lamp 1".
    #[serde(default)]
    pub room: String,
}

/// A rule the driver found already configured on the system it is setting up.
///
/// A Hue bridge, a Lutron processor and a Control4 project all arrive with automations on them —
/// somebody paired that dimmer to those lights, and pairing it is what that *meant*. Throwing that
/// away and asking the household to describe it again is the same waste as ignoring which room
/// each bulb is in, and a good deal more annoying, because a rule is harder to describe than a
/// room.
///
/// Everything here is **late-bound**, and it has to be: a rule refers to bindings, and no binding
/// exists until the installer has adopted something. So a driver points at its own offered
/// devices, by their position in [`SetupStep::Done`]'s list, and at rooms by name. Core resolves
/// both after adoption and refuses anything that does not land.
///
/// What core does with one is the other half of the bargain: it arrives **disabled**, and tagged
/// with where it came from. An imported rule is a driver's reading of somebody else's automation,
/// and the vendor's semantics are never quite these ones — so it is a proposal on the automations
/// page with its origin written on it, not something that starts running in a house at midnight
/// because a bridge was adopted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportedRule {
    /// What to call it. Prefer whatever the far side called it — a name somebody chose beats a
    /// name generated from what the rule does.
    pub label: String,
    /// Which offered device starts it: an index into [`SetupStep::Done`]'s `devices`.
    #[serde(default)]
    pub when_device: usize,
    /// Which of that device's proxies — the `[[proxy]] id` from the manifest. A multi-sensor's
    /// motion binding rather than its temperature one.
    #[serde(default)]
    pub when_proxy: LocalId,
    /// Notification parameters that must match, for a proxy that carries several of something.
    ///
    /// A keypad is one binding with its keys as parameters, so the proxy id alone cannot say
    /// *which* key a suggested rule is about — `{ "key": 2 }` is what distinguishes the Up
    /// button from the Off one. Empty means any, which is right for everything that has one of
    /// whatever it is.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub when_params: std::collections::BTreeMap<String, serde_json::Value>,
    /// The notifications that start it: `clicked`, `held`. Each is checked against the contract.
    ///
    /// Several, because one button often means one intention through more than one event. A
    /// brighter button steps on `clicked` and ramps on `repeating`, and those are the same rule —
    /// splitting them into two would put two lines on the automations page that a household would
    /// have to know to enable together.
    #[serde(default)]
    pub when_events: Vec<String>,
    /// A state key instead of an event, for a rule that starts on a sensor rather than a press.
    /// Mutually exclusive with `when_events`.
    #[serde(default)]
    pub when_key: String,
    /// The value that key must reach. Defaults to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_becomes: Option<Value>,
    pub then: Vec<ImportedAction>,
}

/// A named arrangement the far side already has — a Hue scene.
///
/// The other half of what a commissioned hub knows. Somebody sat and got a room right, named it
/// "Relax", and that is a thing no amount of describing reproduces: it is five lights at five
/// levels and two color temperatures, and the value of it is precisely the detail.
///
/// Late-bound like [`ImportedRule`], and for the same reason — a scene names bindings, and no
/// binding exists until the installer has adopted something. `steps` point at the offered devices
/// by position, and core resolves them once the batch is in.
///
/// Unlike a rule, a scene arrives *live*, because a scene does nothing until somebody asks for it.
/// There is no equivalent of a rule quietly running at midnight, so there is nothing to hold back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportedScene {
    pub title: String,
    /// The rooms it covers, by name. Empty for one that spans the house.
    ///
    /// Several, because a scene does not always follow walls — an open plan is one space and two
    /// rooms, and a grouping the far side made may be neither. Names a room does not have here
    /// are dropped rather than created: nothing of that room was adopted, so the scene has
    /// nothing to do in it.
    #[serde(default)]
    pub rooms: Vec<String>,
    pub steps: Vec<ImportedAction>,
    /// Provider-side scene identity. Its presence makes this a borrowed, read-only resource in
    /// Core: Juno may recall it but must never update or delete it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<ImportedSceneResource>,
}

/// The provider-side identity and recall capabilities of an imported scene.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportedSceneResource {
    pub resource: String,
    #[serde(default)]
    pub dynamic_palette: bool,
}

/// A provider-owned scene discovered after its controller was already adopted.
///
/// Setup-time [`ImportedScene`] actions point into the candidates being adopted. A running
/// controller has no such list, so each step instead names the stable device properties the
/// driver originally installed (for example a Hue light resource id). Core resolves those only
/// among children of the controller that emitted the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BorrowedSceneSnapshot {
    pub title: String,
    pub resource: String,
    #[serde(default)]
    pub dynamic_palette: bool,
    #[serde(default)]
    pub steps: Vec<BorrowedSceneStep>,
}

/// One installed-device state represented by a borrowed provider scene.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BorrowedSceneStep {
    /// Stable device properties that must all match one child of the reporting controller.
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    #[serde(default)]
    pub proxy: LocalId,
    pub command: String,
    #[serde(default)]
    pub args: Args,
}

/// One thing an [`ImportedRule`] does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportedAction {
    /// A command on a room, named rather than numbered. Resolved against the rooms that exist by
    /// the time the batch has been adopted — including any the candidates themselves created.
    Room {
        room: String,
        command: String,
        #[serde(default)]
        args: Args,
    },
    /// A command on one of the offered devices, by its position in the same list.
    Device {
        device: usize,
        #[serde(default)]
        proxy: LocalId,
        command: String,
        #[serde(default)]
        args: Args,
    },
}

/// One screen of a driver's setup flow.
///
/// This is what lets a driver ship its own wizard — "press the link button", "here are your
/// bulbs, pick which to add" — without core or the UI knowing anything about the vendor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SetupStep {
    /// Say something and wait for the installer to continue.
    Instruct {
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        continue_label: String,
    },
    /// Ask for values.
    Form {
        title: String,
        #[serde(default)]
        body: String,
        fields: Vec<Field>,
    },
    /// Choose one row from a table — several things were found and they differ in ways a
    /// dropdown cannot show. A table can carry a name, an address and a model side by side,
    /// which is what someone needs to tell two bridges apart.
    Pick {
        title: String,
        #[serde(default)]
        body: String,
        columns: Vec<String>,
        rows: Vec<PickRow>,
        /// The input key the chosen row's `value` is returned under.
        field: String,
        /// A way to answer by hand when the right thing is not listed. Multicast is blocked
        /// on plenty of networks, so this is not optional in practice.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        manual: Option<Field>,
    },
    /// Offer what was found.
    Choose {
        title: String,
        #[serde(default)]
        body: String,
        options: Vec<Candidate>,
        #[serde(default)]
        multiple: bool,
    },
    /// Core should make this request and re-enter with the response in `input.response`.
    /// The driver never touches a socket; core owns every timeout and retry.
    Fetch {
        request: HttpRequest,
        #[serde(default)]
        note: String,
    },
    /// Exchange bytes over a connection core holds open across several steps.
    ///
    /// [`Self::Fetch`] is one request and one response, which cannot express a protocol where
    /// the device speaks first. A Lutron bridge pushes its button-press confirmation before it
    /// will accept a signing request, and opening a second connection misses it; so does any
    /// protocol that greets you, or that answers with acknowledgements ahead of the reply you
    /// asked for.
    ///
    /// The driver still never touches a socket. It says where to connect, what to send, and
    /// how long to listen; core owns the connection, the TLS, and the deadline. What came back
    /// arrives as text in `input.received`, with the connection's id in `input.session` —
    /// pass that back to keep using it. **Framing is the driver's job**: core returns whatever
    /// arrived within the window and does not know where one message ends.
    ///
    /// # Binary protocols
    ///
    /// `send`/`received` are text, and a pairing handshake made of encrypted frames cannot use
    /// them: `received` is built with a lossy UTF-8 decode, so a ChaCha20 ciphertext arrives
    /// with most of its bytes replaced and no way to tell. Use [`Self::Session::send_bytes`] and
    /// read `input.received_bytes` instead — the same connection, the same step, bytes end to
    /// end. They are separate fields rather than a mode flag so that a driver cannot half-switch
    /// and get a silently mangled handshake.
    ///
    /// Connections live as long as the run of steps that opened them, and close on their own
    /// when the flow next needs a person or finishes.
    Session {
        /// An id from a previous step's `input.session`. Absent opens a new connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<u32>,
        /// Where to connect. Required when opening, ignored when continuing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        open: Option<Connect>,
        /// Wait for the device to connect to *us* instead. See [`Accept`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accept: Option<Accept>,
        /// Written before listening. Empty listens without sending, which is how you receive
        /// a greeting the device sends unprompted.
        #[serde(default)]
        send: String,
        /// The same, as bytes, for a protocol that is not text. What came back arrives in
        /// `input.received_bytes` as an array of numbers, undecoded.
        ///
        /// Set both this and `send` and this one wins — core writes bytes and answers with
        /// bytes. Nothing warns about it because there is no sensible reason to set both, and a
        /// warning at pairing time is read long after the flow has moved on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        send_bytes: Vec<u8>,
        /// How long to listen. Core returns everything that arrived in the window, which may
        /// be nothing.
        #[serde(default = "half_second")]
        read_ms: u32,
        /// Close afterwards. A flow that ends without this still closes; saying so is for
        /// devices that allow one connection at a time.
        #[serde(default)]
        close: bool,
        #[serde(default)]
        note: String,
    },
    /// Core should make a client identity and re-enter with it in `input`.
    ///
    /// The third step core acts on itself, and here for the same reason as [`Self::Fetch`]:
    /// so drivers do not each ship their own. A pairing certificate is the only crypto any
    /// driver here has ever needed, and doing it in the driver cost 291 KB — `rcgen`, `ring`,
    /// and seven crates behind them, statically linked for four lines of use, every one of
    /// which the controller already had. A driver is a separate library, so nothing it links
    /// is shared with anything; a `SetupStep` is.
    ///
    /// Comes back as `input.key_pem` and `input.csr_pem`. The private key is generated in the
    /// controller and handed to the driver in the flow state, which is the same trust the
    /// driver already has — it is the thing that will present the certificate. What it is
    /// spared is the code that makes one.
    ///
    /// Nothing about the key type is offered. Devices that demand mutual TLS during pairing
    /// accept what the controller emits, and a knob here would be a way to pick something
    /// weaker rather than something that works.
    MakeIdentity {
        /// The certificate's common name, and the CSR's subject.
        #[serde(default)]
        common_name: String,
        #[serde(default)]
        note: String,
    },
    /// Nothing to do yet — try again shortly. Waiting for a link button press.
    Wait {
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default = "one_second")]
        retry_ms: u32,
    },
    /// Finished. These devices are confirmed and ready to adopt.
    Done {
        devices: Vec<Candidate>,
        /// Automations the far side already has, offered for import. Created disabled and tagged
        /// with their origin — see [`ImportedRule`]. Empty for every driver that does not look.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rules: Vec<ImportedRule>,
        /// Named arrangements the far side already has — see [`ImportedScene`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scenes: Vec<ImportedScene>,
    },
    /// Could not continue, and why.
    Failed { reason: String },
}

impl SetupStep {
    /// Finished, with nothing to import.
    ///
    /// The overwhelmingly common ending, and the one worth a constructor. `rules` and `scenes`
    /// were added to [`SetupStep::Done`] later and broke every driver that had written the
    /// variant out by hand — four of five, all with the same two-line diff. [`Candidate`]
    /// derives `Default` to head off exactly that, but an enum variant cannot, so this is where
    /// the same protection has to live. A driver that does import something writes it itself.
    pub fn done(devices: Vec<Candidate>) -> SetupStep {
        SetupStep::Done {
            devices,
            rules: Vec::new(),
            scenes: Vec::new(),
        }
    }
}

fn one_second() -> u32 {
    1000
}

fn half_second() -> u32 {
    500
}

/// Where a [`SetupStep::Session`] should connect, and how.
///
/// Deliberately says nothing about what is spoken over it. Core opens the socket, does the
/// handshake, and hands back bytes; every protocol above that belongs to the driver that
/// understands it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Connect {
    pub host: String,
    pub port: u16,
    /// Wrap in TLS.
    ///
    /// A device on your own network is accepted on the strength of the pairing secret it gave
    /// you, not its certificate — bridges present self-signed certificates no public CA has
    /// heard of, so verifying against the public roots would reject every one of them.
    #[serde(default)]
    pub tls: bool,
    /// Present a client certificate during the handshake, for a device that demands mutual
    /// TLS. Both must be PEM, and both must be set for either to be used.
    ///
    /// These never leave the process — core reads them straight out of this struct — so a
    /// per-installation key does not go on any wire to get here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
}

/// Wait for the device to connect to **us**, optionally saying so over mDNS.
///
/// The inverse of [`Connect`], and it is not a symmetry for its own sake: some pairing flows only
/// happen inbound. A Qolsys panel pairs a touchscreen by scanning for a service, dialling it, and
/// signing the certificate request it finds there — the panel is the client and the thing being
/// paired is the server. A driver that could only dial out could never be paired at all.
///
/// Core owns the listener, the advertisement and the TLS, and tears all three down when the flow
/// moves on. What arrives comes back in `input.received` and the connection stays open under the
/// same `input.session` as any other, so the rest of the exchange is written exactly like an
/// outbound one.
///
/// The driver never sees an address to guess at: `port` may be zero, core picks a free one, and
/// the advertisement carries whichever it got.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Accept {
    /// Service type to advertise while listening — `_http._tcp`. Empty advertises nothing, for a
    /// device that already knows where to look.
    #[serde(default)]
    pub mdns_type: String,
    /// Instance name within that type. The device is usually looking for an exact one.
    #[serde(default)]
    pub mdns_name: String,
    /// 0 asks for any free port, which is what an advertised service should do — a fixed one is
    /// a clash waiting for the second controller on the network.
    #[serde(default)]
    pub port: u16,
    /// Present TLS. The certificate is ours to choose and the far side will not verify it: in
    /// these flows it is about to *issue* us one.
    #[serde(default)]
    pub tls: bool,
    /// PEM identity to present, both or neither. [`SetupStep::MakeIdentity`] makes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Connect {
    pub fn tcp(host: impl Into<String>, port: u16) -> Connect {
        Connect {
            host: host.into(),
            port,
            ..Default::default()
        }
    }

    pub fn tls(host: impl Into<String>, port: u16) -> Connect {
        Connect {
            host: host.into(),
            port,
            tls: true,
            ..Default::default()
        }
    }

    /// Mutual TLS, presenting `cert`/`key` during the handshake.
    pub fn mutual_tls(
        host: impl Into<String>,
        port: u16,
        cert: impl Into<String>,
        key: impl Into<String>,
    ) -> Connect {
        Connect {
            host: host.into(),
            port,
            tls: true,
            client_cert: Some(cert.into()),
            client_key: Some(key.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub calls: Vec<HostCall>,
    /// The driver's per-device memory after the call. Core writes it back to the instance.
    #[serde(default)]
    pub scratch: BTreeMap<String, Value>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    /// The next screen of a setup flow, when the request was `Discover` or `Setup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<SetupStep>,
    /// Flow state to hand back on the next call. Core stores it; the driver stays stateless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    /// Native group handling result, when the request was [`Request::OnGroup`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupResponse>,
    /// Native scene handling result, when the request was [`Request::OnScene`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneResponse>,
}

/// Run a request against any [`DriverModule`]. Shared by the in-process path and every
/// out-of-process runtime, so all of them behave identically.
pub fn dispatch(module: &dyn DriverModule, request: Request) -> Response {
    let (calls, instance) = match request {
        Request::OnAction {
            driver_id: _,
            action,
            args,
            mut instance,
        } => {
            let calls = module.on_action(&mut instance, &action, &args);
            (calls, Some(instance))
        }
        Request::OnCommand {
            driver_id: _,
            proxy,
            cmd,
            args,
            mut instance,
        } => {
            let calls = module.on_command(&mut instance, proxy, &cmd, &args);
            (calls, Some(instance))
        }
        Request::OnGroup {
            driver_id: _,
            request,
            mut instance,
        } => {
            let group = module.on_group(&mut instance, &request);
            return Response {
                scratch: instance.scratch,
                group: Some(group),
                ..Default::default()
            };
        }
        Request::OnScene {
            driver_id: _,
            request,
            mut instance,
        } => {
            let scene = module.on_scene(&mut instance, &request);
            return Response {
                scratch: instance.scratch,
                scene: Some(scene),
                ..Default::default()
            };
        }
        Request::OnNodeCommand {
            driver_id: _,
            node,
            kind,
            cmd,
            args,
            mut instance,
        } => {
            let calls = module.on_node_command(&mut instance, &node, &kind, &cmd, &args);
            (calls, Some(instance))
        }
        Request::OnEvent {
            driver_id: _,
            control,
            note,
            args,
            mut instance,
        } => {
            let calls = module.on_event(&mut instance, control, &note, &args);
            (calls, Some(instance))
        }
        Request::OnBind {
            driver_id: _,
            mut instance,
        } => {
            let calls = module.on_bind(&mut instance);
            (calls, Some(instance))
        }
        Request::Unsupported => {
            return Response {
                unsupported: module.unsupported(),
                ..Default::default()
            };
        }
        Request::Discover {
            driver_id,
            state,
            input,
        } => {
            let (step, next) = module.discover(&driver_id, &state, &input);
            return Response {
                step: Some(step),
                state: Some(next),
                ..Default::default()
            };
        }
        Request::Setup {
            driver_id,
            state,
            input,
        } => {
            let (step, next) = module.setup(&driver_id, &state, &input);
            return Response {
                step: Some(step),
                state: Some(next),
                ..Default::default()
            };
        }
    };
    Response {
        calls,
        scratch: instance.map(|i| i.scratch).unwrap_or_default(),
        ..Default::default()
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    struct Provider;

    impl DriverModule for Provider {
        fn on_command(
            &self,
            _inst: &mut Instance,
            _proxy: LocalId,
            _cmd: &str,
            _args: &Args,
        ) -> Vec<HostCall> {
            Vec::new()
        }

        fn on_group(&self, inst: &mut Instance, request: &GroupRequest) -> GroupResponse {
            inst.scratch
                .insert("seen_group".into(), serde_json::json!(request.group));
            GroupResponse {
                disposition: GroupDisposition::Handled,
                status: serde_json::json!({ "members": request.members.len() }),
                ..Default::default()
            }
        }
    }

    #[test]
    fn group_dispatch_round_trips_the_result_and_provider_scratch() {
        let response = dispatch(
            &Provider,
            Request::OnGroup {
                driver_id: "test.provider".into(),
                request: GroupRequest {
                    group: 7,
                    name: "Lights".into(),
                    state: Args::new(),
                    members: Vec::new(),
                    operation: GroupOperation::Status,
                },
                instance: Instance::new(1),
            },
        );
        assert_eq!(
            response.group.unwrap().disposition,
            GroupDisposition::Handled
        );
        assert_eq!(
            response.scratch.get("seen_group"),
            Some(&serde_json::json!(7))
        );
    }
}

#[cfg(test)]
mod scene_tests {
    use super::*;

    struct Provider;

    impl DriverModule for Provider {
        fn on_command(
            &self,
            _inst: &mut Instance,
            _proxy: LocalId,
            _cmd: &str,
            _args: &Args,
        ) -> Vec<HostCall> {
            Vec::new()
        }

        fn on_scene(&self, inst: &mut Instance, request: &SceneRequest) -> SceneResponse {
            inst.scratch
                .insert("seen_scene".into(), serde_json::json!(request.scene));
            SceneResponse {
                disposition: GroupDisposition::Handled,
                status: serde_json::json!({ "ownership": request.ownership }),
                ..Default::default()
            }
        }
    }

    #[test]
    fn scene_dispatch_round_trips_result_and_provider_scratch() {
        let response = dispatch(
            &Provider,
            Request::OnScene {
                driver_id: "test.provider".into(),
                request: SceneRequest {
                    scene: 9,
                    name: "Evening".into(),
                    ownership: SceneOwnership::Juno,
                    resource: None,
                    members: Vec::new(),
                    animation: SceneAnimation::default(),
                    operation: SceneOperation::Status,
                },
                instance: Instance::new(1),
            },
        );
        assert_eq!(
            response.scene.unwrap().disposition,
            GroupDisposition::Handled
        );
        assert_eq!(
            response.scratch.get("seen_scene"),
            Some(&serde_json::json!(9))
        );
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    /// A driver compiles the SDK **without** the `contracts` feature, so the types it needs to
    /// answer [`HostCall::Connections`] have to live outside `manifest`. This test is the guard:
    /// it builds one the way a driver does and round-trips it through the ABI's JSON, which is
    /// what a wasm driver's reply actually crosses.
    #[test]
    fn a_driver_can_build_and_serialize_connections() {
        let call = HostCall::Connections {
            connections: vec![ConnectionDecl {
                id: 1001,
                proxy: 2,
                dir: Direction::Consumer,
                class: "HDMI".into(),
                name: "HDMI-1".into(),
            }],
        };
        let wire = serde_json::to_string(&call).expect("serializes");
        assert!(wire.contains("\"call\":\"connections\""), "{wire}");
        assert!(wire.contains("\"dir\":\"consumer\""), "{wire}");
        assert_eq!(serde_json::from_str::<HostCall>(&wire).unwrap(), call);
    }

    /// Empty is a real answer — "this device has no signal connections" — and must survive the
    /// round trip as itself rather than collapsing into something core reads as absent.
    #[test]
    fn an_empty_list_is_still_an_answer() {
        let call = HostCall::Connections {
            connections: Vec::new(),
        };
        let back: HostCall = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back, call);
    }
}
