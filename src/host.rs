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
    Tx { control: LocalId, data: Vec<u8> },
    /// An HTTP request. Core owns the client so drivers cannot each ship their own, and so
    /// timeouts, retries, and TLS are enforced in one place.
    Http(HttpRequest),
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
    Log { level: String, msg: String },
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
/// boundary is undefined behaviour waiting to happen; a serialised call is slower and
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
    /// Look for devices of this kind on the network. The driver knows how to recognise its
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
/// levels and two colour temperatures, and the value of it is precisely the detail.
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
    /// arrives as raw bytes in `input.received`, with the connection's id in `input.session` —
    /// pass that back to keep using it. **Framing is the driver's job**: core returns whatever
    /// arrived within the window and does not know where one message ends.
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
        /// Written before listening. Empty listens without sending, which is how you receive
        /// a greeting the device sends unprompted.
        #[serde(default)]
        send: String,
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
