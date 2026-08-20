//! SDDP — the announcement protocol a great deal of AV hardware already speaks, and the
//! matching rules a driver writes against it.
//!
//! Both halves live here because both are contract. [`Found`] is the shape of what a device
//! puts on the wire, and [`SddpMatch`] is what a manifest's `[discovery]` block claims from
//! it; a driver author needs the first to write the second. What is *not* here is the
//! listener — receiving multicast is a controller's job, not a contract.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One device's announcement.
///
/// Every header is kept, not just the ones matching knows about: the protocol is a vendor's
/// and the set of fields is theirs to change, so throwing away what we did not recognize
/// would mean a driver author cannot match on something real hardware is plainly sending.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// Where it announced from — what a driver will actually talk to.
    pub address: String,
    /// `Host`, the device's own name for itself.
    #[serde(default)]
    pub host: String,
    /// `Type`, e.g. `sony:display`.
    #[serde(default)]
    pub ty: String,
    /// `Primary-Proxy` — the device class it wants driving as. Close enough to a Juno proxy
    /// name to be worth surfacing, though the vocabularies are not the same.
    #[serde(default)]
    pub primary_proxy: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub model: String,
    /// `Driver` — the filename of the driver the device expects a controller to hold. An
    /// opaque label here, and the most specific thing a device tells you about itself.
    #[serde(default)]
    pub driver: String,
    /// Everything else it sent, lowercased keys.
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}


/// What a driver claims from an SDDP announcement.
///
/// Every field is optional and every one that is set must match — so a rule is as loose or as
/// tight as the driver author needs, from "any Sony display" to one exact model. Values are
/// globs: `*` stands for any run of characters, because the thing most worth matching on is a
/// driver filename and those carry the model in them.
///
/// A bare string is shorthand for `{ type = "..." }`, which is what the hints were before
/// there was anything else to match on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "SddpMatchRepr", into = "SddpMatchRepr")]
pub struct SddpMatch {
    /// `Type`, e.g. `sony:display`.
    pub ty: Option<String>,
    /// `Driver` — the filename the device says a controller should hold for it.
    ///
    /// The most specific thing a device volunteers, and the reason this exists: a great deal
    /// of AV hardware announces a Control4 driver filename whether or not a Control4 system is
    /// listening, and that filename names the exact model. Matching it is how our own driver
    /// claims a device we would otherwise have no way to recognize.
    ///
    /// It is an opaque label. Nothing reads, ships or derives from the file it names.
    pub driver: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    /// `Primary-Proxy` — the device class it wants driving as.
    pub primary_proxy: Option<String>,
}

/// How a match is written down: either a bare type string, or a table of fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum SddpMatchRepr {
    Type(String),
    Fields {
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
        ty: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        driver: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        manufacturer: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        primary_proxy: Option<String>,
    },
}

impl From<SddpMatchRepr> for SddpMatch {
    fn from(r: SddpMatchRepr) -> Self {
        match r {
            SddpMatchRepr::Type(ty) => SddpMatch {
                ty: Some(ty),
                ..Default::default()
            },
            SddpMatchRepr::Fields {
                ty,
                driver,
                manufacturer,
                model,
                primary_proxy,
            } => SddpMatch {
                ty,
                driver,
                manufacturer,
                model,
                primary_proxy,
            },
        }
    }
}

impl From<SddpMatch> for SddpMatchRepr {
    fn from(m: SddpMatch) -> Self {
        SddpMatchRepr::Fields {
            ty: m.ty,
            driver: m.driver,
            manufacturer: m.manufacturer,
            model: m.model,
            primary_proxy: m.primary_proxy,
        }
    }
}

impl SddpMatch {
    /// Whether an announcement satisfies every field this rule sets.
    ///
    /// A rule with no fields set matches nothing. Matching everything would mean one careless
    /// driver claiming every device in the house.
    pub fn matches(&self, seen: &Found) -> bool {
        let checks = [
            (self.ty.as_deref(), seen.ty.as_str()),
            (self.driver.as_deref(), seen.driver.as_str()),
            (self.manufacturer.as_deref(), seen.manufacturer.as_str()),
            (self.model.as_deref(), seen.model.as_str()),
            (self.primary_proxy.as_deref(), seen.primary_proxy.as_str()),
        ];
        let mut asked = false;
        for (pattern, value) in checks {
            let Some(pattern) = pattern else { continue };
            asked = true;
            // A field the device did not send cannot satisfy a rule that asks about it —
            // not even `*`. Otherwise a rule reading "any model" would claim a device that
            // never mentioned a model, which is the opposite of what it says.
            if value.is_empty() || !glob_match(pattern, value) {
                return false;
            }
        }
        asked
    }
}

/// `*` stands for any run of characters; everything else is literal and case-insensitive.
///
/// Enough for driver filenames and model numbers, which is all this matches. A full regex
/// would let a driver author write one that takes exponential time on an attacker-supplied
/// announcement, and nothing here needs the expressiveness.
pub(crate) fn glob_match(pattern: &str, value: &str) -> bool {
    let (pattern, value) = (pattern.to_ascii_lowercase(), value.to_ascii_lowercase());
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut rest = &value[first.len()..];
    let mut last: Option<&str> = None;
    for part in parts {
        last = Some(part);
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    // A pattern not ending in `*` has to reach the end of the value.
    match last {
        None => rest.is_empty(),
        Some(part) if !pattern.ends_with('*') => rest.is_empty() || part.is_empty(),
        Some(_) => true,
    }
}
