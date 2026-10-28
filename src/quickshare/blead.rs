//! The bytes phones look for over Bluetooth LE to find a receiver.
//!
//! A phone that searches over Bluetooth only (Quick Share's "Share with
//! Apple devices") lists receivers from service data under the 16-bit UUID
//! `0xFEF3`. Two forms exist (layouts from google/nearby,
//! `connections/implementation/ble_advertisement.cc` and
//! `mediums/ble/ble_advertisement{,_header}.cc`):
//!
//! - the *medium advertisement* ([`medium_advertisement`]): endpoint id and
//!   endpoint info, sent in an extended advertisement and served from GATT;
//! - the 17-byte *header* ([`header`]) for legacy advertisements, which only
//!   tells the phone to read the medium advertisement over GATT.

use sha2::{Digest, Sha256};

use super::endpoint::{EndpointInfo, SERVICE_HASH};

/// Connections advertisement byte 0: version 1 (3 bits), PCP 3
/// (P2P_POINT_TO_POINT, 5 bits).
const CONNECTIONS_VERSION_PCP: u8 = (1 << 5) | 3;
/// Medium advertisement byte 0: version 2 (3 bits), socket version 2 (3
/// bits), not fast, no second profile.
const MEDIUM_VERSION: u8 = (2 << 5) | (2 << 2);
/// Header byte 0: version 2 (3 bits), the extended flag, one GATT slot.
const HEADER_VERSION_SLOTS: u8 = (2 << 5) | 1;
const HEADER_EXTENDED: u8 = 1 << 4;
/// The 80-bit bloom filter with only "NearbySharing" in it.
const BLOOM: [u8; 10] = [0x20, 0x00, 0x00, 0x10, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00];
/// The connections advertisement allows at most 131 bytes of endpoint info:
/// byte 0, 16 random bytes, the length and 113 bytes of name.
const MAX_NAME: usize = 113;

/// The regular medium advertisement of endpoint `endpoint_id` with `info`
/// (a fresh endpoint info, like the one in the mDNS TXT record `n`, with the
/// name cut to fit). It carries no Bluetooth Classic address and no L2CAP
/// PSM, so a phone connects over the GATT socket.
pub fn medium_advertisement(endpoint_id: [u8; 4], info: &EndpointInfo) -> Vec<u8> {
    let name = info.name.as_deref().map(|n| {
        let mut end = n.len().min(MAX_NAME);
        while !n.is_char_boundary(end) {
            end -= 1;
        }
        n[..end].to_owned()
    });
    let info = EndpointInfo { kind: info.kind, name }.encode();
    medium(endpoint_id, &info, rand::random())
}

/// The medium advertisement around the connections advertisement of
/// `endpoint_id` and `endpoint_info`, with a 2-byte device `token` (random,
/// as Google's advertiser sends one) and no extra fields.
fn medium(endpoint_id: [u8; 4], endpoint_info: &[u8], token: [u8; 2]) -> Vec<u8> {
    let mut connections = vec![CONNECTIONS_VERSION_PCP];
    connections.extend(SERVICE_HASH);
    connections.extend(endpoint_id);
    connections.push(endpoint_info.len() as u8);
    connections.extend(endpoint_info);
    // Bluetooth Classic MAC (all zero: none), UWB address length, extra
    // field (not WebRTC connectable).
    connections.extend([0; 6]);
    connections.extend([0, 0]);

    let mut out = vec![MEDIUM_VERSION];
    out.extend(SERVICE_HASH);
    out.extend((connections.len() as u32).to_be_bytes());
    out.extend(connections);
    out.extend(token);
    out
}

/// The header for a legacy advertisement: one GATT slot holding `medium`,
/// `extended` if the same advertisement is also on the air as an extended
/// one, a hash that changes with `medium` and no L2CAP PSM.
pub fn header(medium: &[u8], extended: bool) -> [u8; 17] {
    let mut out = [0; 17];
    out[0] = HEADER_VERSION_SLOTS | if extended { HEADER_EXTENDED } else { 0 };
    out[1..11].copy_from_slice(&BLOOM);
    out[11..15].copy_from_slice(&Sha256::digest(medium)[..4]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quickshare::endpoint::DeviceKind;

    fn bytes(hex: &str) -> Vec<u8> {
        let hex: String = hex.split_whitespace().collect();
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    /// The worked example of the protocol notes: name "Luna-PC", endpoint
    /// "ABCD", laptop, salt `1122`, key `AA`×14, token `BEEF`.
    fn example() -> Vec<u8> {
        bytes(
            "48 FC9F5E 0000002A
             23 FC9F5E 41424344 19
             06 1122 AAAAAAAAAAAAAAAAAAAAAAAAAAAA 07 4C756E612D5043
             000000000000 00 00
             BEEF",
        )
    }

    #[test]
    fn medium_advertisement_layout() {
        let info = bytes("06 1122 AAAAAAAAAAAAAAAAAAAAAAAAAAAA 07 4C756E612D5043");
        assert_eq!(medium(*b"ABCD", &info, [0xBE, 0xEF]), example());

        let laptop = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("nixos".into()),
        };
        let adv = medium_advertisement(*b"ABCD", &laptop);
        assert_eq!(adv.len(), 45 + 5);
        assert_eq!(adv[..8], bytes("48 FC9F5E 00000028")[..]);
        assert_eq!(adv[8..17], bytes("23 FC9F5E 41424344 17")[..]);
        assert_eq!(EndpointInfo::decode(&adv[17..40]), Some(laptop));
        assert_eq!(adv[40..48], [0; 8], "no MAC, no UWB address, not WebRTC connectable");
    }

    #[test]
    fn medium_advertisement_cuts_long_names_to_fit() {
        let long = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("ä".repeat(100)),
        };
        let adv = medium_advertisement(*b"ABCD", &long);
        let info_len = adv[16] as usize;
        assert!(info_len <= 131, "{info_len} bytes of endpoint info");
        let name = EndpointInfo::decode(&adv[17..17 + info_len]).unwrap().name.unwrap();
        assert_eq!(name, "ä".repeat(56));
        assert_eq!(adv.len(), 8 + 17 + info_len + 2);
    }

    #[test]
    fn header_layout() {
        assert_eq!(header(&example(), false)[..], bytes("41 20000010020000030000 D7D8CFAA 0000")[..]);
        assert_eq!(header(&example(), true)[..], bytes("51 20000010020000030000 D7D8CFAA 0000")[..]);
        assert_ne!(header(&example()[1..], false)[11..15], header(&example(), false)[11..15]);
    }
}
