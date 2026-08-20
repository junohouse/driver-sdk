//! What a driver claims from an mDNS advertisement.
//!
//! A service type alone is often enough — `_hue._tcp` is a Hue bridge and nothing else. It is
//! sometimes nowhere near enough, and the gap is not academic: `_airplay._tcp` is advertised by
//! every AirPlay 2 receiver on the network, which is Apple TVs, televisions from three other
//! manufacturers, speakers, and the laptop you are configuring from. A driver that declared it
//! offered itself for all of them.
//!
//! The Apple TV manifest had already written down what it actually wanted:
//!
//! ```text
//! # `_companion-link._tcp` is the one that matters — it is the service that carries control.
//! # `_airplay._tcp` is browsed too because its TXT record names the model, which is the only
//! # way to tell two identically-named televisions apart before either has been set up.
//! ```
//!
//! It had no way to say it, so it said "mine" instead and core believed it. This is the way to
//! say it, and it is declarative for the same reason [`crate::sddp::SddpMatch`] is: a driver
//! author writes what their hardware looks like, and nothing about a new device needs a change
//! in core.

use crate::sddp::glob_match;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A service type, and optionally what its TXT record has to say.
///
/// A bare string is shorthand for the service on its own, which is what every hint was before
/// there was anything else to match on — so every manifest and every published index row keeps
/// working untouched:
///
/// ```toml
/// mdns = [
///   "_hue._tcp",
///   { service = "_airplay._tcp", txt = { model = "AppleTV*" } },
/// ]
/// ```
///
/// Values are globs, like SDDP's, and for the same reason: the useful thing to match is a
/// model string and those carry a family plus a revision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "MdnsMatchRepr", into = "MdnsMatchRepr")]
pub struct MdnsMatch {
    /// `_hue._tcp`. Always set — it is what core browses for.
    pub service: String,
    /// Keys the advertisement must carry, and what they must look like. Empty means the
    /// service type alone is the claim.
    pub txt: BTreeMap<String, String>,
}

impl MdnsMatch {
    /// Whether an advertisement satisfies this rule.
    ///
    /// A TXT key the device did not send fails the rule, `*` included — a driver asking about
    /// a model is asking for hardware that states one, and treating silence as a match is how
    /// `_airplay._tcp` came to mean "Apple TV" in the first place.
    pub fn matches(&self, service: &str, txt: &BTreeMap<String, String>) -> bool {
        if !self.service.eq_ignore_ascii_case(service) {
            return false;
        }
        self.txt.iter().all(|(key, pattern)| {
            txt.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .is_some_and(|(_, value)| !value.is_empty() && glob_match(pattern, value))
        })
    }
}

impl From<&str> for MdnsMatch {
    fn from(service: &str) -> Self {
        MdnsMatch {
            service: service.to_string(),
            txt: BTreeMap::new(),
        }
    }
}

/// How a rule is written down: a bare service type, or a table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum MdnsMatchRepr {
    Service(String),
    Fields {
        service: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        txt: BTreeMap<String, String>,
    },
}

impl From<MdnsMatchRepr> for MdnsMatch {
    fn from(r: MdnsMatchRepr) -> Self {
        match r {
            MdnsMatchRepr::Service(service) => MdnsMatch {
                service,
                txt: BTreeMap::new(),
            },
            MdnsMatchRepr::Fields { service, txt } => MdnsMatch { service, txt },
        }
    }
}

impl From<MdnsMatch> for MdnsMatchRepr {
    fn from(m: MdnsMatch) -> Self {
        // Written back the way it was most likely written down. A round trip through the
        // registry must not turn every plain service type into a one-key table, or the first
        // index regeneration rewrites every driver's hints into a form nobody typed.
        if m.txt.is_empty() {
            MdnsMatchRepr::Service(m.service)
        } else {
            MdnsMatchRepr::Fields {
                service: m.service,
                txt: m.txt,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_bare_service_type_still_means_the_service_type() {
        let rule: MdnsMatch = serde_json::from_str("\"_hue._tcp\"").expect("a published index row");
        assert!(rule.matches("_hue._tcp", &BTreeMap::new()));
        assert!(!rule.matches("_airplay._tcp", &BTreeMap::new()));
    }

    /// The case this exists for: three devices, one service type, one of them an Apple TV.
    #[test]
    fn a_txt_constraint_separates_devices_sharing_a_service() {
        let rule: MdnsMatch = serde_json::from_str(
            r#"{ "service": "_airplay._tcp", "txt": { "model": "AppleTV*" } }"#,
        )
        .expect("parses");

        assert!(rule.matches("_airplay._tcp", &txt(&[("model", "AppleTV14,1")])));
        assert!(
            !rule.matches("_airplay._tcp", &txt(&[("model", "Hisense_R6")])),
            "a Roku television is not an Apple TV",
        );
        assert!(
            !rule.matches("_airplay._tcp", &txt(&[("srcvers", "770.8.1")])),
            "a device that never said what it is cannot satisfy a rule about what it is",
        );
    }

    /// A round trip must not rewrite hints nobody wrote that way.
    #[test]
    fn a_plain_service_type_survives_a_round_trip_as_a_plain_string() {
        let rule = MdnsMatch::from("_hue._tcp");
        assert_eq!(serde_json::to_string(&rule).unwrap(), "\"_hue._tcp\"");
    }
}
