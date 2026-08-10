//! The shape of a `proxies/*.toml` file, and the structural checks that run on load.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Bool,
    U8,
    U32,
    I32,
    F32,
    String,
    StringList,
    /// Opaque bytes. Carried as a hex string on the wire; used only by transport proxies.
    Bytes,
    /// A structure the contract deliberately does not describe: a page of browse results, a
    /// search hit list. Every other type exists so a control can be drawn and an argument
    /// checked without reading the driver — this one exists for payloads whose shape belongs
    /// to a music service rather than to us, and which are rendered as a list either way.
    ///
    /// Arrays and objects only. Allowing scalars would make `json` a synonym for "any" and
    /// take the rest of the contract's validation down with it.
    Json,
}

impl ValueType {
    /// Implicit bounds that come from the type itself, before any `min`/`max` on the param.
    pub(crate) fn intrinsic_range(self) -> Option<(f64, f64)> {
        match self {
            ValueType::U8 => Some((0.0, 255.0)),
            ValueType::U32 => Some((0.0, u32::MAX as f64)),
            ValueType::I32 => Some((i32::MIN as f64, i32::MAX as f64)),
            _ => None,
        }
    }

    pub fn accepts(self, v: &Value) -> bool {
        match (self, v) {
            (ValueType::Bool, Value::Bool(_)) => true,
            (ValueType::U8 | ValueType::U32, Value::Number(n)) => n.as_u64().is_some(),
            (ValueType::I32, Value::Number(n)) => n.as_i64().is_some(),
            (ValueType::F32, Value::Number(_)) => true,
            (ValueType::String | ValueType::Bytes, Value::String(_)) => true,
            (ValueType::StringList, Value::Array(a)) => a.iter().all(Value::is_string),
            (ValueType::Json, Value::Array(_) | Value::Object(_)) => true,
            _ => false,
        }
    }
}

/// A feature flag or limit a driver declares. Commands and notifications gate on these.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub default: Value,
    #[serde(default)]
    pub doc: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default)]
    pub optional: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Allowed values. Only meaningful for `string`.
    #[serde(default)]
    pub values: Vec<String>,
    /// Capabilities that narrow this parameter for a particular device. `kelvin` spans
    /// 1000-10000 across all lights, but a given bulb declares `cct_min`/`cct_max` — and
    /// both the AI's view and the validation gate must use the bulb's numbers, not the
    /// proxy's. Otherwise we advertise a range the hardware silently clamps.
    pub min_cap: Option<String>,
    pub max_cap: Option<String>,
    /// State key holding this parameter's valid values, when they are discovered from the
    /// device rather than fixed in the contract. A Roku's installed apps are not knowable
    /// when the proxy is written, but the assistant still has to be told what it can ask for.
    pub values_from: Option<String>,
    #[serde(default)]
    pub doc: String,
}

impl Param {
    /// The effective range for one device: contract bounds, narrowed by its capabilities.
    pub fn range(&self, caps: &BTreeMap<String, Value>) -> (Option<f64>, Option<f64>) {
        let from_cap = |name: &Option<String>| {
            name.as_ref()
                .and_then(|n| caps.get(n))
                .and_then(Value::as_f64)
        };
        (
            from_cap(&self.min_cap).or(self.min),
            from_cap(&self.max_cap).or(self.max),
        )
    }
}

/// Commands and notifications share a shape: docs, an optional capability gate, and params.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    /// What a person calls this — "Turn off", "Brightness changed". The wire name stays
    /// `off`; nobody building a rule should have to read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub doc: String,
    /// Name of a `bool` capability. Absent from a resolved contract unless the driver
    /// declared this capability true.
    pub requires: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateField {
    #[serde(rename = "type")]
    pub ty: ValueType,
    /// What a person calls this value — "Brightness" rather than `level`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub doc: String,
    /// Notification parameter that feeds this state key, when the two are not named the same.
    /// `temperature_c` is fed by `temperature_changed{celsius}`, so it says `from = "celsius"`.
    ///
    /// State that cannot be derived from a single parameter — a setpoint whose meaning depends
    /// on a sibling `which` field — is set explicitly by the driver with
    /// [`crate::driver::HostCall::SetState`] instead.
    pub from: Option<String>,
    /// A boolean core maintains from another key on the same binding: true when that key is
    /// non-zero, non-empty, not false.
    ///
    /// A light is *on* when its level is above zero, and every consumer wants that as a
    /// boolean — the tile that draws a toggle, the rule that triggers on it, the miner looking
    /// for a habit. Declared here so the contract stays data and no driver has to remember to
    /// report a second key that is a function of the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthy_of: Option<String>,
    /// A boolean core maintains from another key being one of a set of values.
    ///
    /// The sibling of `truthy_of`, for the case where truthiness is the wrong question. Weather is
    /// *wet* when the condition is rain, sleet, hail or a storm — and `"clear"` is a perfectly
    /// truthy string, so `truthy_of` would say it was raining on a sunny day.
    ///
    /// Declared rather than computed for the same reason: a rule, a tile and the miner must not
    /// each arrive at their own idea of what counts as rain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_of: Option<OneOf>,
}

/// "This boolean is true when that key is one of these."
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OneOf {
    /// The key to read.
    pub key: String,
    /// The values that make this true. Anything else, including a missing value, makes it false.
    pub values: Vec<String>,
}

/// `level_changed` → `Level changed`. What a name nobody has titled should read as, so a
/// contract that has not been gone through yet is still legible in the editor.
pub fn humanize(name: &str) -> String {
    let mut out = name.replace('_', " ");
    if out.is_ascii() && !out.is_empty() {
        out[..1].make_ascii_uppercase();
    }
    out
}

impl Signature {
    /// What to show a person for this command or notification.
    pub fn label(&self, name: &str) -> String {
        self.title.clone().unwrap_or_else(|| humanize(name))
    }
}

impl StateField {
    /// What to show a person for this value.
    pub fn label(&self, key: &str) -> String {
        self.title.clone().unwrap_or_else(|| humanize(key))
    }

    /// The notification parameter to read for this state key.
    pub fn source<'a>(&'a self, key: &'a str) -> &'a str {
        self.from.as_deref().unwrap_or(key)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    pub name: String,
    pub version: u32,
    pub title: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: BTreeMap<String, Capability>,
    #[serde(default)]
    pub commands: BTreeMap<String, Signature>,
    #[serde(default)]
    pub notifications: BTreeMap<String, Signature>,
    #[serde(default)]
    pub state: BTreeMap<String, StateField>,
}

impl Proxy {
    pub fn parse(src: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(src)
    }

    /// Structural problems with the contract itself. Returns every error, not just the first,
    /// because a proxy author wants the whole list.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();

        if self.name.is_empty() {
            errs.push("name must not be empty".into());
        }

        for (cap_name, cap) in &self.capabilities {
            if !cap.ty.accepts(&cap.default) {
                errs.push(format!(
                    "capability `{cap_name}`: default {} is not a {:?}",
                    cap.default, cap.ty
                ));
            }
        }

        let check = |kind: &str, sigs: &BTreeMap<String, Signature>, errs: &mut Vec<String>| {
            for (sig_name, sig) in sigs {
                if let Some(req) = &sig.requires {
                    match self.capabilities.get(req) {
                        None => errs.push(format!(
                            "{kind} `{sig_name}`: requires unknown capability `{req}`"
                        )),
                        Some(c) if c.ty != ValueType::Bool => errs.push(format!(
                            "{kind} `{sig_name}`: requires `{req}`, which is {:?}, not bool",
                            c.ty
                        )),
                        Some(_) => {}
                    }
                }
                for (p_name, p) in &sig.params {
                    let where_ = format!("{kind} `{sig_name}` param `{p_name}`");
                    if let (Some(lo), Some(hi)) = (p.min, p.max)
                        && lo > hi
                    {
                        errs.push(format!("{where_}: min {lo} > max {hi}"));
                    }
                    if !p.values.is_empty() && p.ty != ValueType::String {
                        errs.push(format!("{where_}: `values` is only valid on a string"));
                    }
                }
            }
        };
        check("command", &self.commands, &mut errs);
        check("notification", &self.notifications, &mut errs);

        errs
    }
}
