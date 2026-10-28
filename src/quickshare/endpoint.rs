//! Quick Share endpoint info (mDNS TXT record `n`) and mDNS instance names.

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use rand::RngExt;

pub const SERVICE_TYPE: &str = "_FC9F5ED42C8A._tcp.local.";

const HIDDEN_BIT: u8 = 0x10;
/// `SHA-256("NearbySharing")[0..3]`, the service id hash in mDNS names,
/// Bluetooth advertisements and Bluetooth socket messages.
pub const SERVICE_HASH: [u8; 3] = [0xFC, 0x9F, 0x5E];
const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Unknown = 0,
    Phone = 1,
    Tablet = 2,
    Laptop = 3,
    Car = 4,
    Foldable = 5,
    Xr = 6,
}

impl DeviceKind {
    pub fn id(self) -> &'static str {
        match self {
            DeviceKind::Unknown => "unknown",
            DeviceKind::Phone => "phone",
            DeviceKind::Tablet => "tablet",
            DeviceKind::Laptop => "laptop",
            DeviceKind::Car => "car",
            DeviceKind::Foldable => "foldable",
            DeviceKind::Xr => "xr",
        }
    }

    fn from_bits(v: u8) -> DeviceKind {
        match v {
            1 => DeviceKind::Phone,
            2 => DeviceKind::Tablet,
            3 => DeviceKind::Laptop,
            4 => DeviceKind::Car,
            5 => DeviceKind::Foldable,
            6 => DeviceKind::Xr,
            _ => DeviceKind::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointInfo {
    pub kind: DeviceKind,
    pub name: Option<String>,
}

impl EndpointInfo {
    /// Byte 0: version(3) | hidden(1) | kind(3) | reserved(1); 16 random
    /// bytes; length-prefixed UTF-8 name (omitted when hidden).
    pub fn encode(&self) -> Vec<u8> {
        let mut first = (self.kind as u8) << 1;
        if self.name.is_none() {
            first |= HIDDEN_BIT;
        }
        let mut out = vec![first];
        let mut random = [0u8; 16];
        rand::rng().fill(&mut random[..]);
        out.extend(random);
        if let Some(name) = &self.name {
            let mut end = name.len().min(255);
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            out.push(end as u8);
            out.extend(&name.as_bytes()[..end]);
        }
        out
    }

    /// Trailing TLV records are ignored.
    pub fn decode(b: &[u8]) -> Option<EndpointInfo> {
        if b.len() < 17 {
            return None;
        }
        let kind = DeviceKind::from_bits((b[0] >> 1) & 7);
        if b[0] & HIDDEN_BIT != 0 {
            return Some(EndpointInfo { kind, name: None });
        }
        let len = *b.get(17)? as usize;
        let name = std::str::from_utf8(b.get(18..18 + len)?).ok()?;
        Some(EndpointInfo {
            kind,
            name: Some(name.to_owned()),
        })
    }
}

pub fn new_endpoint_id() -> [u8; 4] {
    let mut rng = rand::rng();
    std::array::from_fn(|_| ID_ALPHABET[rng.random_range(0..ID_ALPHABET.len())])
}

pub fn instance_name(id: &[u8; 4]) -> String {
    b64(&[0x23, id[0], id[1], id[2], id[3], SERVICE_HASH[0], SERVICE_HASH[1], SERVICE_HASH[2], 0, 0])
}

pub fn parse_instance_name(s: &str) -> Option<[u8; 4]> {
    let b = unb64(s)?;
    if b.len() < 10 || b[0] != 0x23 || b[5..8] != SERVICE_HASH {
        return None;
    }
    let id: [u8; 4] = b[1..5].try_into().ok()?;
    id.iter().all(u8::is_ascii_alphanumeric).then_some(id)
}

/// URL-safe base64 without padding.
pub fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// Decodes URL-safe base64, with or without padding.
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD_INDIFFERENT.decode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_roundtrip() {
        let i = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("luna-laptop".into()),
        };
        let e = i.encode();
        assert_eq!(e[0], 3 << 1);
        assert_eq!(e.len(), 1 + 16 + 1 + 11);
        assert_eq!(EndpointInfo::decode(&e).unwrap(), i);
    }

    #[test]
    fn info_truncates_long_name_on_char_boundary() {
        let n = "ä".repeat(200);
        let e = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some(n),
        }
        .encode();
        assert!(e[17] as usize <= 255);
        assert_eq!(e.len(), 18 + e[17] as usize);
        assert!(std::str::from_utf8(&e[18..]).is_ok());
    }

    #[test]
    fn info_decodes_phone_with_tlv() {
        let mut b = vec![1 << 1];
        b.extend([0u8; 16]);
        b.push(5);
        b.extend(b"Galax");
        b.extend([2, 1, 1]);
        let i = EndpointInfo::decode(&b).unwrap();
        assert_eq!(i.kind, DeviceKind::Phone);
        assert_eq!(i.name.as_deref(), Some("Galax"));
    }

    #[test]
    fn info_hidden_has_no_name() {
        let mut b = vec![(1 << 1) | 0x10];
        b.extend([0u8; 16]);
        let i = EndpointInfo::decode(&b).unwrap();
        assert_eq!(i.name, None);
    }

    #[test]
    fn info_rejects_short() {
        assert!(EndpointInfo::decode(&[2; 10]).is_none());
        let mut b = vec![6];
        b.extend([0u8; 16]);
        b.push(9); // claims 9 name bytes, has none
        assert!(EndpointInfo::decode(&b).is_none());
    }

    #[test]
    fn instance_name_roundtrip() {
        let id = *b"AB12";
        let n = instance_name(&id);
        assert!(!n.contains('='));
        assert_eq!(parse_instance_name(&n), Some(id));
        assert!(parse_instance_name("bm90aGluZw").is_none());
    }

    #[test]
    fn endpoint_id_is_alphanumeric() {
        for _ in 0..50 {
            assert!(new_endpoint_id().iter().all(u8::is_ascii_alphanumeric));
        }
    }

    #[test]
    fn b64_accepts_padding() {
        assert_eq!(b64(&[0xfb, 0xff]), "-_8");
        assert_eq!(unb64("-_8"), Some(vec![0xfb, 0xff]));
        assert_eq!(unb64("-_8="), Some(vec![0xfb, 0xff]));
        assert!(unb64("!!").is_none());
    }

    #[test]
    fn device_kind_ids() {
        assert_eq!(DeviceKind::Phone.id(), "phone");
        assert_eq!(DeviceKind::Xr.id(), "xr");
    }
}
