//! The encrypted channel after UKEY2: every `OfflineFrame` travels inside a
//! `SecureMessage` (AES-256-CBC + HMAC-SHA256, per-direction sequence numbers).

use aes::Aes256;
use anyhow::{Context, bail};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use hmac::{Hmac, KeyInit, Mac};
use prost::Message;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::proto::securegcm::{self, DeviceToDeviceMessage, GcmMetadata};
use super::proto::securemessage::{EncScheme, Header, HeaderAndBody, SecureMessage, SigScheme};
use super::ukey2::Keys;

const IV_LEN: usize = 16;
const METADATA_VERSION: i32 = 1;

/// One side of the encrypted channel, with its own send and receive sequence.
pub struct SecureChannel {
    encrypt: [u8; 32],
    decrypt: [u8; 32],
    send_hmac: [u8; 32],
    recv_hmac: [u8; 32],
    send_seq: i32,
    recv_seq: i32,
}

impl SecureChannel {
    pub fn new(keys: &Keys) -> Self {
        SecureChannel {
            encrypt: keys.encrypt,
            decrypt: keys.decrypt,
            send_hmac: keys.send_hmac,
            recv_hmac: keys.recv_hmac,
            send_seq: 0,
            recv_seq: 0,
        }
    }

    /// Wraps a serialized `OfflineFrame` into a serialized `SecureMessage`.
    pub fn seal(&mut self, offline_frame: &[u8]) -> Vec<u8> {
        // Wrapping only after 2^31 messages; the peer then rejects the number.
        self.send_seq = self.send_seq.wrapping_add(1);
        let d2d = DeviceToDeviceMessage {
            message: Some(offline_frame.to_vec()),
            sequence_number: Some(self.send_seq),
        };
        let iv: [u8; IV_LEN] = rand::random();
        let body = cbc::Encryptor::<Aes256>::new(&self.encrypt.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(&d2d.encode_to_vec());
        let metadata = GcmMetadata {
            r#type: securegcm::Type::DeviceToDeviceMessage as i32,
            version: Some(METADATA_VERSION),
        };
        let header = Header {
            signature_scheme: SigScheme::HmacSha256 as i32,
            encryption_scheme: EncScheme::Aes256Cbc as i32,
            iv: Some(iv.to_vec()),
            public_metadata: Some(metadata.encode_to_vec()),
            ..Header::default()
        };
        let header_and_body = HeaderAndBody { header, body }.encode_to_vec();
        SecureMessage {
            signature: hmac(&self.send_hmac, &header_and_body).to_vec(),
            header_and_body,
        }
        .encode_to_vec()
    }

    /// Checks and decrypts a serialized `SecureMessage`, returning the
    /// serialized `OfflineFrame` inside. The signature is checked over the
    /// `header_and_body` bytes exactly as received, before anything is
    /// decrypted; the sequence number must be the next one expected.
    pub fn open(&mut self, secure_message: &[u8]) -> anyhow::Result<Vec<u8>> {
        let msg = SecureMessage::decode(secure_message).context("decoding SecureMessage")?;
        let expected = hmac(&self.recv_hmac, &msg.header_and_body);
        if !bool::from(expected.as_slice().ct_eq(&msg.signature)) {
            bail!("SecureMessage signature does not match");
        }
        let hb = HeaderAndBody::decode(msg.header_and_body.as_slice()).context("decoding HeaderAndBody")?;
        if hb.header.encryption_scheme != EncScheme::Aes256Cbc as i32 || hb.header.signature_scheme != SigScheme::HmacSha256 as i32 {
            bail!("SecureMessage uses an unsupported scheme");
        }
        let iv: [u8; IV_LEN] = hb
            .header
            .iv
            .as_deref()
            .and_then(|iv| iv.try_into().ok())
            .context("SecureMessage IV is not 16 bytes")?;
        let plain = cbc::Decryptor::<Aes256>::new(&self.decrypt.into(), &iv.into())
            .decrypt_padded_vec::<Pkcs7>(&hb.body)
            .map_err(|_| anyhow::anyhow!("SecureMessage body does not decrypt"))?;
        let d2d = DeviceToDeviceMessage::decode(plain.as_slice()).context("decoding DeviceToDeviceMessage")?;
        let next = self.recv_seq.wrapping_add(1);
        match d2d.sequence_number {
            Some(seq) if seq == next => {}
            seq => bail!("unexpected sequence number {seq:?}, expected {next}"),
        }
        let frame = d2d.message.context("DeviceToDeviceMessage without message")?;
        self.recv_seq = next;
        Ok(frame)
    }
}

fn hmac(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::super::ukey2::{Role, derive};
    use super::*;

    fn pair() -> (SecureChannel, SecureChannel) {
        let shared = [7u8; 32];
        let server = derive(&shared, b"client init", b"server init", Role::Server);
        let client = derive(&shared, b"client init", b"server init", Role::Client);
        (SecureChannel::new(&server), SecureChannel::new(&client))
    }

    #[test]
    fn seal_open_roundtrip() {
        let (mut server, mut client) = pair();
        let first = server.seal(b"x");
        assert_eq!(client.open(&first).unwrap(), b"x");
        let second = server.seal(b"x");
        assert_ne!(first, second, "fresh IV and sequence number each time");
        assert_eq!(client.open(&second).unwrap(), b"x");

        let back = client.seal(b"reply");
        assert_eq!(server.open(&back).unwrap(), b"reply");
    }

    #[test]
    fn seal_uses_the_wire_format() {
        let (mut server, _) = pair();
        let msg = SecureMessage::decode(server.seal(b"frame").as_slice()).unwrap();
        assert_eq!(msg.signature.len(), 32);
        let hb = HeaderAndBody::decode(msg.header_and_body.as_slice()).unwrap();
        assert_eq!(hb.header.encryption_scheme, EncScheme::Aes256Cbc as i32);
        assert_eq!(hb.header.signature_scheme, SigScheme::HmacSha256 as i32);
        assert_eq!(hb.header.iv.as_deref().map(<[u8]>::len), Some(16));
        let md = GcmMetadata::decode(hb.header.public_metadata.unwrap().as_slice()).unwrap();
        assert_eq!(md.r#type, securegcm::Type::DeviceToDeviceMessage as i32);
        assert_eq!(md.version, Some(1));
        assert_eq!(hb.body.len() % 16, 0);
    }

    #[test]
    fn open_rejects_tampered() {
        let (mut server, mut client) = pair();
        let mut msg = SecureMessage::decode(server.seal(b"x").as_slice()).unwrap();
        let last = msg.header_and_body.len() - 1;
        msg.header_and_body[last] ^= 1;
        assert!(client.open(&msg.encode_to_vec()).is_err());
    }

    #[test]
    fn open_rejects_replay() {
        let (mut server, mut client) = pair();
        let msg = server.seal(b"x");
        assert_eq!(client.open(&msg).unwrap(), b"x");
        assert!(client.open(&msg).is_err());
    }

    #[test]
    fn open_rejects_wrong_direction() {
        let (mut server, _) = pair();
        let msg = server.seal(b"x");
        assert!(server.open(&msg).is_err(), "own send key must not verify");
    }

    #[test]
    fn open_verifies_the_bytes_as_received() {
        // Same HeaderAndBody, but with the fields in non-canonical order
        // (body before header): a re-encoding would differ from these bytes.
        let (mut server, mut client) = pair();
        let sealed = SecureMessage::decode(server.seal(b"x").as_slice()).unwrap();
        let hb = HeaderAndBody::decode(sealed.header_and_body.as_slice()).unwrap();
        let mut raw = Vec::new();
        prost::encoding::bytes::encode(2, &hb.body, &mut raw);
        prost::encoding::message::encode(1, &hb.header, &mut raw);
        assert_ne!(raw, hb.encode_to_vec());
        let msg = SecureMessage {
            signature: hmac(&server.send_hmac, &raw).to_vec(),
            header_and_body: raw,
        };
        assert_eq!(client.open(&msg.encode_to_vec()).unwrap(), b"x");
    }
}
