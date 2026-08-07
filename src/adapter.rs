//! The adapter wire protocol.
//!
//! An adapter is a protocol stack in its own process, speaking newline-delimited JSON on
//! stdin and stdout. These are the frames.
//!
//! They live here rather than in the controller for the reason this whole crate exists: an
//! adapter that had to hand-copy them across a version boundary would be one field rename
//! away from silently disagreeing with the controller, and nothing would catch it. Owning
//! them here means both ends are the same definition.
//!
//! Every type derives **both** `Serialize` and `Deserialize`, because each side reads what the
//! other writes: the controller deserialises [`Up`] and serialises [`Down`], and an adapter
//! does the mirror image.

use crate::host::{Args, HostCall};
use crate::LocalId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire protocol version, sent in [`Up::Hello`].
///
/// Declared independently on both sides, exactly as `ABI_VERSION` already is across the cdylib
/// boundary. A controller accepts `PROTOCOL` and `PROTOCOL - 1`, and that one rule is what
/// makes an adapter upgrade and a controller upgrade two maintenance windows instead of one.
pub const PROTOCOL: u32 = 1;


/// Adapter to core. One JSON object per line on stdout.
///
/// Deliberately **not** `deny_unknown_fields`, and an unrecognised tag is logged and dropped
/// rather than fatal — the opposite of [`crate::driver::manifest::Manifest`]. A manifest is
/// written by a person and a typo there should fail loudly at install. This is written by a
/// program on the other side of a version boundary, and a newer adapter adding a field must not
/// brick an older core in somebody's house at 9pm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "f", rename_all = "snake_case")]
pub enum Up {
    /// First line after spawn. Nothing else is accepted before it.
    Hello {
        protocol: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        version: String,
    },
    /// One coordinator's whole inventory, as it currently is.
    ///
    /// A snapshot rather than add/remove deltas, because a delta protocol needs a resync path
    /// for the restart case and a resync path is a snapshot protocol with extra steps. After a
    /// crash the adapter simply says what it has, and core reconciles.
    Present {
        /// Which coordinator this is the inventory of.
        coord: u32,
        nodes: Vec<Node>,
    },
    /// Something happened on a device. `calls` are ordinary [`HostCall`]s — the same ones an
    /// in-process driver returns, which is what makes a Zigbee bulb indistinguishable from a Hue
    /// bulb from `Runtime::emit` upward.
    Push {
        coord: u32,
        /// The coordinator's own id for the node, as given in [`Node::node`]. Unique within a
        /// coordinator and nowhere else.
        node: String,
        calls: Vec<HostCall>,
    },
    /// The outcome of a [`Down::Command`] or [`Down::Action`], matched by `token`.
    ///
    /// Commands are fire-and-forget, so this is how a caller learns anything at all. Absence is
    /// not failure — plenty of adapters will never send one.
    Result {
        token: u64,
        ok: bool,
        #[serde(default)]
        detail: String,
    },
    /// Configuration the adapter owns and core must keep.
    ///
    /// The half that closes the loop. `Down::Open` hands an adapter what it was left with; this
    /// is how anything new gets back. A Zigbee coordinator forming a network for the first time
    /// mints its own network key, and a key that never reaches the project is a mesh that
    /// cannot be restored onto a replacement radio — every securely-joined device would need
    /// pairing again, on foot, with the label in hand.
    ///
    /// Core does not read it. It is the adapter's own shape, stored under (driver, coordinator)
    /// and handed straight back on the next `Open`. Storing something one cannot interpret is
    /// the point: the alternative is core learning a schema that changes whenever the adapter
    /// does.
    Config {
        coord: u32,
        config: Value,
    },
    /// A coordinator came up, or went away. Reported rather than inferred: a radio that has
    /// stopped answering is a different thing from one that was never configured, and an
    /// integrator needs to be told which.
    Status {
        coord: u32,
        online: bool,
        #[serde(default)]
        detail: String,
    },
    /// Something worth showing an integrator. Kept separate from stderr so an adapter can be
    /// deliberate about what surfaces in the UI versus what is only in the log.
    Log {
        #[serde(default)]
        level: String,
        message: String,
    },
}

/// One device an adapter is offering, already mapped to Juno's semantics.
///
/// The mapping from a Zigbee cluster or a Z-Wave command class to a proxy contract lives in the
/// **adapter**, never here. Core has no idea what cluster `0x0006` is and must never learn:
/// that knowledge changes with every firmware and every quirk, and putting it in core would mean
/// shipping a controller release to support a new bulb.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Node {
    /// Stable, adapter-assigned. An IEEE address or a Z-Wave node id — something that survives
    /// the device being renamed, re-roomed, or the adapter restarting.
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
    /// one product: a plain white bulb and an extended-colour bulb are both `light`, and if they
    /// resolve to the same contract then `set_cct` appears in the UI, in the automation editor
    /// and in the assistant's tool surface for a bulb that cannot do it — and `validate_call`
    /// waves it through to a driver that fails in silence. Somebody then spends an evening
    /// deciding their new bulb is faulty.
    ///
    /// The adapter is the only thing that can fill this in honestly, because the answer lives in
    /// `zigbee-herdsman-converters` — a decade of per-model capability data that is exactly the
    /// driver work nobody should repeat. Core does not have it and should never learn it: that
    /// database changes weekly, and baking it in would mean a controller release per new bulb.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Value>,
    /// Whether the adapter can currently reach it. A battery sensor that has not reported is
    /// not a fault, so this is shown rather than acted on.
    #[serde(default = "yes")]
    pub online: bool,
    /// Where the far side says this device lives. Empty when it has no idea, which is the
    /// normal case for a radio.
    ///
    /// A **suggestion**, and the distinction is the whole reason this is safe. Rooms belong to
    /// the project (`house::project::Room`), and nothing here creates one: an offered node
    /// carries the name through to the moment an installer adopts it, and core matches or
    /// creates only then, with the list on screen. A driver still cannot make a room while
    /// nobody is looking, rename one, or delete one.
    ///
    /// It exists because some systems genuinely know. A Zigbee mesh does not, but a Control4
    /// project does — every device in it is already filed under a room somebody named — and
    /// throwing that away would mean hand-placing several hundred devices to import a house
    /// that had already been commissioned once.
    #[serde(default)]
    pub room: String,
}

fn yes() -> bool {
    true
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
        }
    }
}

/// Core to adapter. One JSON object per line on stdin.
///
/// Absent fields default rather than failing the frame. This direction used to be
/// serialise-only, so it never had to read anything; now that both sides share one definition
/// it does, and a controller a version behind must not produce frames an adapter refuses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "f", rename_all = "snake_case")]
pub enum Down {
    /// Sent once, after [`Up::Hello`] is accepted. Carries the driver-owned configuration held
    /// in the project — a network key, a node database — so the adapter can come up in the
    /// state it was left in rather than forming a new network and orphaning the house.
    Configure { protocol: u32 },
    /// Bring up one coordinator. Sent once per coordinator after [`Down::Configure`], and again
    /// whenever one is added to the project.
    ///
    /// `config` is that coordinator's own driver-owned state from the project — its network key,
    /// its node database. Per coordinator and never shared: two Zigbee networks have two keys,
    /// and mixing them up means a mesh nobody can rejoin.
    Open {
        coord: u32,
        /// `host:port`. These radios are on the network.
        address: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        config: Option<Value>,
        /// The coordinator's properties, as the installer set them.
        ///
        /// `config` is what the *adapter* last saved; this is what a person typed. Both are
        /// needed and neither substitutes for the other: a Zigbee network key is minted by the
        /// radio and comes back through [`Up::Config`], but a shared secret for a bridge on
        /// somebody else's appliance is typed into a form and has no other way in.
        ///
        /// Sent whole rather than as a named credential field, because which properties matter
        /// is the driver's business — the manifest declared them.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        props: BTreeMap<String, Value>,
    },
    /// Take one coordinator down, leaving the process and any other coordinators alone.
    Close { coord: u32 },
    /// A proxy command for one node. `token` correlates the eventual [`Up::Result`].
    Command {
        token: u64,
        coord: u32,
        node: String,
        proxy: LocalId,
        cmd: String,
        #[serde(default)]
        args: Args,
    },
    /// A driver-declared action — `permit_join`, `heal`. See
    /// [`crate::driver::manifest::ActionDecl`] for why these are not commands.
    Action {
        token: u64,
        coord: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        action: String,
        #[serde(default)]
        args: Args,
    },
    /// Asked to stop. The adapter has a grace period to flush and exit before it is killed.
    Shutdown,
}
