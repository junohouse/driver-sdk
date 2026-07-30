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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Nothing to do yet — try again shortly. Waiting for a link button press.
    Wait {
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default = "one_second")]
        retry_ms: u32,
    },
    /// Finished. These devices are confirmed and ready to adopt.
    Done { devices: Vec<Candidate> },
    /// Could not continue, and why.
    Failed { reason: String },
}

fn one_second() -> u32 {
    1000
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
