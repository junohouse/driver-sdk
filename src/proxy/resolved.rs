//! Narrowing a contract to one driver, and validating traffic across that boundary.
//!
//! Two gates live here, and everything in the system passes through one of them:
//!
//! - [`Proxy::validate_call`] — commands going *in*, from the UI, an automation, or the AI.
//! - [`Proxy::validate_notification`] — notifications coming *out* of a driver.

use super::schema::{Param, Proxy, Signature, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A proxy contract narrowed to what one driver actually implements. Owned, so core can keep
/// one per binding without borrowing the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolved {
    pub proxy_type: String,
    pub caps: BTreeMap<String, Value>,
    /// Commands this device actually supports, in contract order.
    pub commands: Vec<String>,
    pub notifications: Vec<String>,
}

impl Resolved {
    pub fn supports(&self, command: &str) -> bool {
        self.commands.iter().any(|c| c == command)
    }

    pub fn emits(&self, notification: &str) -> bool {
        self.notifications.iter().any(|n| n == notification)
    }

    /// Capability value, or `Value::Null` if the proxy has no such capability.
    pub fn cap(&self, name: &str) -> &Value {
        self.caps.get(name).unwrap_or(&Value::Null)
    }
}

#[derive(Debug, PartialEq)]
pub enum CallError {
    /// The proxy has no such command at all — a bug in the caller.
    NoSuchCommand(String),
    /// The command exists but this device did not declare the capability it needs.
    Unsupported {
        command: String,
        requires: String,
    },
    MissingParam(String),
    UnknownParam(String),
    BadType {
        param: String,
        expected: ValueType,
        got: Value,
    },
    OutOfRange {
        param: String,
        value: f64,
        min: f64,
        max: f64,
    },
    NotAllowed {
        param: String,
        got: String,
        allowed: Vec<String>,
    },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NoSuchCommand(c) => write!(f, "no such command `{c}`"),
            CallError::Unsupported { command, requires } => {
                write!(f, "`{command}` needs capability `{requires}`, not declared")
            }
            CallError::MissingParam(p) => write!(f, "missing required param `{p}`"),
            CallError::UnknownParam(p) => write!(f, "unknown param `{p}`"),
            CallError::BadType {
                param,
                expected,
                got,
            } => write!(f, "param `{param}`: {got} is not a {expected:?}"),
            CallError::OutOfRange {
                param,
                value,
                min,
                max,
            } => write!(f, "param `{param}`: {value} outside {min}..={max}"),
            CallError::NotAllowed {
                param,
                got,
                allowed,
            } => write!(f, "param `{param}`: `{got}` not one of {allowed:?}"),
        }
    }
}

impl std::error::Error for CallError {}

impl Proxy {
    /// Merge a driver's declared capabilities over the defaults and compute what is callable.
    pub fn resolve(&self, declared: &BTreeMap<String, Value>) -> Result<Resolved, Vec<String>> {
        let mut errs = Vec::new();
        let mut caps: BTreeMap<String, Value> = self
            .capabilities
            .iter()
            .map(|(k, c)| (k.clone(), c.default.clone()))
            .collect();

        for (k, v) in declared {
            match self.capabilities.get(k) {
                None => errs.push(format!(
                    "unknown capability `{k}` for proxy `{}`",
                    self.name
                )),
                Some(spec) if !spec.ty.accepts(v) => {
                    errs.push(format!("capability `{k}`: {v} is not a {:?}", spec.ty))
                }
                Some(_) => {
                    caps.insert(k.clone(), v.clone());
                }
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }

        let enabled = |sig: &Signature| match &sig.requires {
            None => true,
            Some(req) => caps.get(req).and_then(Value::as_bool).unwrap_or(false),
        };
        let commands = self
            .commands
            .iter()
            .filter(|(_, s)| enabled(s))
            .map(|(k, _)| k.clone())
            .collect();
        let notifications = self
            .notifications
            .iter()
            .filter(|(_, s)| enabled(s))
            .map(|(k, _)| k.clone())
            .collect();

        Ok(Resolved {
            proxy_type: self.name.clone(),
            caps,
            commands,
            notifications,
        })
    }

    /// Gate for every command, whatever issued it — UI, automation, or AI.
    pub fn validate_call(
        &self,
        resolved: &Resolved,
        command: &str,
        args: &BTreeMap<String, Value>,
    ) -> Result<(), CallError> {
        let sig = self
            .commands
            .get(command)
            .ok_or_else(|| CallError::NoSuchCommand(command.to_string()))?;

        if !resolved.supports(command) {
            return Err(CallError::Unsupported {
                command: command.to_string(),
                requires: sig.requires.clone().unwrap_or_default(),
            });
        }
        check_params(sig, args, &resolved.caps)
    }

    /// Gate for notifications coming *out* of a driver. A driver emitting something its
    /// declared capabilities do not cover has over-declared or mis-wired something, and we
    /// would rather catch that here than in a living room.
    pub fn validate_notification(
        &self,
        resolved: &Resolved,
        name: &str,
        args: &BTreeMap<String, Value>,
    ) -> Result<(), CallError> {
        let sig = self
            .notifications
            .get(name)
            .ok_or_else(|| CallError::NoSuchCommand(name.to_string()))?;

        if !resolved.emits(name) {
            return Err(CallError::Unsupported {
                command: name.to_string(),
                requires: sig.requires.clone().unwrap_or_default(),
            });
        }
        check_params(sig, args, &resolved.caps)
    }
}

fn check_params(
    sig: &Signature,
    args: &BTreeMap<String, Value>,
    caps: &BTreeMap<String, Value>,
) -> Result<(), CallError> {
    for (name, spec) in &sig.params {
        match args.get(name) {
            None if spec.optional => continue,
            None => return Err(CallError::MissingParam(name.clone())),
            Some(v) => check_param(name, spec, v, caps)?,
        }
    }
    for name in args.keys() {
        if !sig.params.contains_key(name) {
            return Err(CallError::UnknownParam(name.clone()));
        }
    }
    Ok(())
}

fn check_param(
    name: &str,
    spec: &Param,
    v: &Value,
    caps: &BTreeMap<String, Value>,
) -> Result<(), CallError> {
    if !spec.ty.accepts(v) {
        return Err(CallError::BadType {
            param: name.to_string(),
            expected: spec.ty,
            got: v.clone(),
        });
    }

    if let Some(n) = v.as_f64() {
        // Intrinsic type bounds first, then whatever the contract narrowed them to.
        let (mut lo, mut hi) = spec.ty.intrinsic_range().unwrap_or((f64::MIN, f64::MAX));
        let (cmin, cmax) = spec.range(caps);
        if let Some(m) = cmin {
            lo = m;
        }
        if let Some(m) = cmax {
            hi = m;
        }
        if n < lo || n > hi {
            return Err(CallError::OutOfRange {
                param: name.to_string(),
                value: n,
                min: lo,
                max: hi,
            });
        }
    }

    if !spec.values.is_empty()
        && let Some(s) = v.as_str()
        && !spec.values.iter().any(|a| a == s)
    {
        return Err(CallError::NotAllowed {
            param: name.to_string(),
            got: s.to_string(),
            allowed: spec.values.clone(),
        });
    }

    Ok(())
}
