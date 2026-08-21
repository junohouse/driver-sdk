//! UDP discovery — a broadcast a vendor's own app makes, and the rules a manifest writes
//! against what comes back.
//!
//! The fourth way to find something, and the one the other three cannot replace. mDNS, SSDP
//! and SDDP are all *standards*: a device that speaks none of them is invisible to all of
//! them, and plenty of the most ordinary hardware in a house speaks none of them. TP-Link's
//! Tapo and Kasa lines are the case that forced this — they answer a broadcast on a port of
//! the vendor's own choosing, in a format of the vendor's own choosing, and that is the only
//! thing they answer. Home Assistant finds them exactly this way and nothing else does.
//!
//! Like the other three, this is a *transport* capability and not a driver's business: a
//! manifest declares the port, the payload and what a reply of its own looks like, and core
//! does the sending and the listening. Drivers still never open a socket.
//!
//! ```toml
//! [[discovery.udp]]
//! port = 20002
//! # A binary query, so hex. `send` is there for the vendors whose query is text.
//! send_hex = "020000010200000000000000463cb5d3"
//! expect = "mgt_encrypt_schm"
//! ```
//!
//! # Why this matches the whole index and [`crate::manifest::Probe`] does not
//!
//! A probe is an outbound connection to *every address on the network*, so core only ever
//! sweeps ports an installed driver asked about — installing is the consent. This is one
//! datagram to the broadcast address, which is what SSDP's `M-SEARCH` already is and what the
//! vendor's own phone app does every time it opens. So it runs for the whole registry index,
//! like the other three, and a controller with nothing installed still finds the dimmer.
//!
//! # The reply reaches the driver
//!
//! Unlike SSDP, there is no header vocabulary to pick apart here — a vendor's reply is a
//! vendor's own format. So [`Found::reply`] is carried whole to the driver's `discover`, and
//! what it means is the driver's business. That is where the real prize is: TP-Link's reply
//! names the model, the MAC and *which encryption scheme the device wants*, which is the
//! difference between a driver that guesses at a protocol variant and one that is told.

use serde::{Deserialize, Serialize};

/// One device's answer to a broadcast.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// Where it answered from — what a driver will actually talk to.
    pub address: String,
    /// The port the query went to, so a driver declaring two can tell them apart.
    pub port: u16,
    /// Everything it sent, undecoded. A vendor's format is the vendor's to change, and half a
    /// reply is no use to a driver trying to read a model number out of it.
    pub reply: Vec<u8>,
}

/// What to broadcast, and what makes a reply this driver's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UdpMatch {
    pub port: u16,
    /// Broadcast as text, for a vendor whose query is a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<String>,
    /// The same, as hex, for a query that is not text — a length-prefixed header, a checksum.
    /// Set both and this one wins; they are separate fields rather than a mode flag for the
    /// reason [`crate::host::SetupStep::Session`] gives, which is that a half-switched driver
    /// gets a silently mangled payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_hex: Option<String>,
    /// A reply is this driver's if it contains this. Matched as a plain substring against the
    /// reply decoded lossily, not as a pattern — the same rule, and the same reasoning, as
    /// [`crate::manifest::Probe::expect`].
    ///
    /// Without one, *any* reply on the port is the claim, which is nearly always too generous:
    /// one broadcast on TP-Link's discovery port is answered by every Kasa plug and every Tapo
    /// device in the house, and they do not take the same driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

impl UdpMatch {
    /// The bytes to put on the wire, or `None` if the manifest's hex will not decode.
    pub fn payload(&self) -> Option<Vec<u8>> {
        match (&self.send_hex, &self.send) {
            (Some(hex), _) => unhex(hex),
            (None, Some(text)) => Some(text.as_bytes().to_vec()),
            // Nothing to say. Some vendors answer an empty datagram; most do not, and a rule
            // that sends nothing is at worst a listen that finds nothing.
            (None, None) => Some(Vec::new()),
        }
    }

    pub fn matches(&self, found: &Found) -> bool {
        if self.port != found.port {
            return false;
        }
        match &self.expect {
            Some(marker) => String::from_utf8_lossy(&found.reply).contains(marker.as_str()),
            None => true,
        }
    }
}

/// Hex to bytes, tolerating the separators people put in captured payloads.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b':' && *b != b'-')
        .collect();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_claims_its_own_port_and_its_own_reply() {
        let rule = UdpMatch {
            port: 20002,
            send_hex: Some("02 00 00 01".into()),
            expect: Some("mgt_encrypt_schm".into()),
            ..Default::default()
        };
        assert_eq!(rule.payload(), Some(vec![0x02, 0x00, 0x00, 0x01]));

        let reply = |port, body: &str| Found {
            address: "10.0.0.4".into(),
            port,
            reply: body.as_bytes().to_vec(),
        };
        assert!(rule.matches(&reply(20002, r#"{"mgt_encrypt_schm":{"encrypt_type":"KLAP"}}"#)));
        // The right port is not the claim. One broadcast here is answered by every TP-Link
        // device in the house, and a Kasa plug does not take a Tapo dimmer's driver.
        assert!(!rule.matches(&reply(20002, r#"{"system":{"get_sysinfo":{}}}"#)));
        assert!(!rule.matches(&reply(9999, r#"{"mgt_encrypt_schm":{}}"#)));

        // Odd digits are a typo in a manifest, and a payload half on the wire finds nothing
        // while looking like it should.
        assert_eq!(UdpMatch { port: 1, send_hex: Some("abc".into()), ..Default::default() }.payload(), None);
        // Text queries stay text.
        let text = UdpMatch { port: 9999, send: Some("hello".into()), ..Default::default() };
        assert_eq!(text.payload(), Some(b"hello".to_vec()));
    }
}
