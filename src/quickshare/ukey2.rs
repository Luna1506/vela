//! UKEY2 handshake (cipher P256_SHA512, next protocol AES_256_CBC-HMAC_SHA256),
//! the key derivation on top of it and the 4-digit PIN shown to the user.

use std::io::{Read, Write};

use anyhow::{Context, anyhow, bail};
use hkdf::Hkdf;
use p256::ecdh::EphemeralSecret;
use p256::elliptic_curve::Generate;
use p256::{PublicKey, Sec1Point};
use prost::Message;
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;

use super::frame::{read_frame, write_frame};
use super::proto::securegcm::ukey2_alert::AlertType;
use super::proto::securegcm::ukey2_message::Type;
use super::proto::securegcm::{Ukey2Alert, Ukey2ClientFinished, Ukey2ClientInit, Ukey2HandshakeCipher, Ukey2Message, Ukey2ServerInit, ukey2_client_init};
use super::proto::securemessage::{EcP256PublicKey, GenericPublicKey, PublicKeyType};

const VERSION: i32 = 1;
const NEXT_PROTOCOL: &str = "AES_256_CBC-HMAC_SHA256";
const RANDOM_LEN: usize = 32;

/// Session keys from the handshake, already oriented for this side.
pub struct Keys {
    pub auth: [u8; 32],
    pub encrypt: [u8; 32],
    pub decrypt: [u8; 32],
    pub send_hmac: [u8; 32],
    pub recv_hmac: [u8; 32],
}

/// Which side of the handshake this end is (the server receives ClientInit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

/// Runs the server side: reads ClientInit, writes ServerInit, reads
/// ClientFinish and checks it against the commitment. A bad ClientInit is
/// answered with a `Ukey2Alert`; a bad ClientFinish is not.
pub fn server(r: &mut impl Read, w: &mut impl Write) -> anyhow::Result<Keys> {
    let client_init = read_frame(r).context("reading UKEY2 ClientInit")?;
    let commitment = check_client_init(&client_init).map_err(|alert| send_alert(w, alert, "ClientInit"))?;

    let secret = EphemeralSecret::generate_from_rng(&mut rand::rng());
    let server_init = Ukey2ServerInit {
        version: Some(VERSION),
        random: Some(rand::random::<[u8; RANDOM_LEN]>().to_vec()),
        handshake_cipher: Some(Ukey2HandshakeCipher::P256Sha512 as i32),
        public_key: Some(encode_public_key(&secret.public_key())),
    };
    let server_init = message(Type::ServerInit, server_init.encode_to_vec());
    write_frame(w, &server_init).context("writing UKEY2 ServerInit")?;

    let finish = read_frame(r).context("reading UKEY2 ClientFinish")?;
    let msg = Ukey2Message::decode(finish.as_slice()).context("decoding UKEY2 ClientFinish")?;
    expect_type(&msg, Type::ClientFinish)?;
    if !bool::from(Sha512::digest(&finish).as_slice().ct_eq(&commitment)) {
        bail!("UKEY2 ClientFinish does not match the ClientInit commitment");
    }
    let finish = Ukey2ClientFinished::decode(msg.message_data.unwrap_or_default().as_slice()).context("decoding UKEY2 ClientFinished")?;
    let peer = decode_public_key(&finish.public_key.unwrap_or_default()).context("client public key")?;

    let shared = secret.diffie_hellman(&peer);
    Ok(derive(&(*shared.raw_secret_bytes()).into(), &client_init, &server_init, Role::Server))
}

/// Runs the client side: writes ClientInit, reads ServerInit, writes
/// ClientFinish. A bad ServerInit is answered with a `Ukey2Alert`.
pub fn client(r: &mut impl Read, w: &mut impl Write) -> anyhow::Result<Keys> {
    let secret = EphemeralSecret::generate_from_rng(&mut rand::rng());
    let finish = Ukey2ClientFinished {
        public_key: Some(encode_public_key(&secret.public_key())),
    };
    let finish = message(Type::ClientFinish, finish.encode_to_vec());

    let client_init = Ukey2ClientInit {
        version: Some(VERSION),
        random: Some(rand::random::<[u8; RANDOM_LEN]>().to_vec()),
        cipher_commitments: vec![ukey2_client_init::CipherCommitment {
            handshake_cipher: Some(Ukey2HandshakeCipher::P256Sha512 as i32),
            commitment: Some(Sha512::digest(&finish).to_vec()),
        }],
        next_protocol: Some(NEXT_PROTOCOL.into()),
    };
    let client_init = message(Type::ClientInit, client_init.encode_to_vec());
    write_frame(w, &client_init).context("writing UKEY2 ClientInit")?;

    let server_init = read_frame(r).context("reading UKEY2 ServerInit")?;
    let peer = check_server_init(&server_init).map_err(|alert| send_alert(w, alert, "ServerInit"))?;

    let shared = secret.diffie_hellman(&peer);
    let keys = derive(&(*shared.raw_secret_bytes()).into(), &client_init, &server_init, Role::Client);
    write_frame(w, &finish).context("writing UKEY2 ClientFinish")?;
    Ok(keys)
}

/// Derives the auth key and the four session keys from the ECDH x coordinate
/// and the two handshake messages exactly as sent (without length prefix).
pub(crate) fn derive(shared_x: &[u8; 32], client_init: &[u8], server_init: &[u8], role: Role) -> Keys {
    let dhs = Sha256::digest(shared_x);
    let info = [client_init, server_init].concat();
    let auth = hkdf(&dhs, b"UKEY2 v1 auth", &info);
    let next = hkdf(&dhs, b"UKEY2 v1 next", &info);

    let d2d_salt = Sha256::digest(b"D2D");
    let d2d_client = hkdf(&next, &d2d_salt, b"client");
    let d2d_server = hkdf(&next, &d2d_salt, b"server");

    let smsg_salt = Sha256::digest(b"SecureMessage");
    let client_key = hkdf(&d2d_client, &smsg_salt, b"ENC:2");
    let client_hmac = hkdf(&d2d_client, &smsg_salt, b"SIG:1");
    let server_key = hkdf(&d2d_server, &smsg_salt, b"ENC:2");
    let server_hmac = hkdf(&d2d_server, &smsg_salt, b"SIG:1");

    match role {
        Role::Server => Keys {
            auth,
            encrypt: server_key,
            decrypt: client_key,
            send_hmac: server_hmac,
            recv_hmac: client_hmac,
        },
        Role::Client => Keys {
            auth,
            encrypt: client_key,
            decrypt: server_key,
            send_hmac: client_hmac,
            recv_hmac: server_hmac,
        },
    }
}

/// The 4-digit PIN both devices show, computed from the auth key.
pub fn pin(auth: &[u8; 32]) -> String {
    let mut hash: i64 = 0;
    let mut mult: i64 = 1;
    for &b in auth {
        hash = (hash + i64::from(b as i8) * mult) % 9973;
        mult = (mult * 31) % 9973;
    }
    format!("{:04}", hash.abs())
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut okm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

fn message(t: Type, data: Vec<u8>) -> Vec<u8> {
    Ukey2Message {
        message_type: Some(t as i32),
        message_data: Some(data),
    }
    .encode_to_vec()
}

/// Sends a `Ukey2Alert` (best effort, the connection is dropped anyway) and
/// returns the error for the caller to propagate.
fn send_alert(w: &mut impl Write, alert: AlertType, what: &str) -> anyhow::Error {
    let body = Ukey2Alert {
        r#type: Some(alert as i32),
        error_message: None,
    };
    let _ = write_frame(w, &message(Type::Alert, body.encode_to_vec()));
    anyhow!("rejected UKEY2 {what}: {}", alert.as_str_name())
}

/// Errors unless `msg` has type `want`; an alert from the peer is reported as such.
fn expect_type(msg: &Ukey2Message, want: Type) -> anyhow::Result<()> {
    match msg.message_type.and_then(|t| Type::try_from(t).ok()) {
        Some(t) if t == want => Ok(()),
        Some(Type::Alert) => {
            let alert = Ukey2Alert::decode(msg.message_data.as_deref().unwrap_or_default()).unwrap_or_default();
            let name = alert.r#type.and_then(|t| AlertType::try_from(t).ok()).map_or("unknown", |t| t.as_str_name());
            bail!("peer sent UKEY2 alert {name} {:?}", alert.error_message.unwrap_or_default())
        }
        other => bail!("expected UKEY2 {}, got {other:?}", want.as_str_name()),
    }
}

/// Validates ClientInit and returns the P256_SHA512 commitment.
fn check_client_init(raw: &[u8]) -> Result<Vec<u8>, AlertType> {
    let msg = Ukey2Message::decode(raw).map_err(|_| AlertType::BadMessage)?;
    if msg.message_type != Some(Type::ClientInit as i32) {
        return Err(AlertType::BadMessageType);
    }
    let init = Ukey2ClientInit::decode(msg.message_data.ok_or(AlertType::BadMessageData)?.as_slice()).map_err(|_| AlertType::BadMessageData)?;
    if init.version != Some(VERSION) {
        return Err(AlertType::BadVersion);
    }
    if init.random.as_ref().map(Vec::len) != Some(RANDOM_LEN) {
        return Err(AlertType::BadRandom);
    }
    let commitment = init
        .cipher_commitments
        .into_iter()
        .find(|c| c.handshake_cipher == Some(Ukey2HandshakeCipher::P256Sha512 as i32))
        .and_then(|c| c.commitment)
        .ok_or(AlertType::BadHandshakeCipher)?;
    if init.next_protocol.as_deref() != Some(NEXT_PROTOCOL) {
        return Err(AlertType::BadNextProtocol);
    }
    Ok(commitment)
}

/// Validates ServerInit and returns the server's public key.
fn check_server_init(raw: &[u8]) -> Result<PublicKey, AlertType> {
    let msg = Ukey2Message::decode(raw).map_err(|_| AlertType::BadMessage)?;
    if msg.message_type != Some(Type::ServerInit as i32) {
        return Err(AlertType::BadMessageType);
    }
    let init = Ukey2ServerInit::decode(msg.message_data.ok_or(AlertType::BadMessageData)?.as_slice()).map_err(|_| AlertType::BadMessageData)?;
    if init.version != Some(VERSION) {
        return Err(AlertType::BadVersion);
    }
    if init.random.as_ref().map(Vec::len) != Some(RANDOM_LEN) {
        return Err(AlertType::BadRandom);
    }
    if init.handshake_cipher != Some(Ukey2HandshakeCipher::P256Sha512 as i32) {
        return Err(AlertType::BadHandshakeCipher);
    }
    decode_public_key(&init.public_key.unwrap_or_default()).map_err(|_| AlertType::BadPublicKey)
}

/// Serialized `GenericPublicKey` (EC_P256) with signed big-endian coordinates.
fn encode_public_key(key: &PublicKey) -> Vec<u8> {
    let point = Sec1Point::from(key);
    let coordinate = |c: Option<&p256::FieldBytes>| signed_be(&(*c.expect("a public key is never the identity")).into());
    GenericPublicKey {
        r#type: PublicKeyType::EcP256 as i32,
        ec_p256_public_key: Some(EcP256PublicKey {
            x: coordinate(point.x()),
            y: coordinate(point.y()),
        }),
        ..Default::default()
    }
    .encode_to_vec()
}

fn decode_public_key(raw: &[u8]) -> anyhow::Result<PublicKey> {
    let key = GenericPublicKey::decode(raw).context("decoding GenericPublicKey")?;
    if key.r#type != PublicKeyType::EcP256 as i32 {
        bail!("public key type {} is not EC_P256", key.r#type);
    }
    let ec = key.ec_p256_public_key.context("EC_P256 public key missing")?;
    let x = unsigned_32(&ec.x).context("x coordinate out of range")?;
    let y = unsigned_32(&ec.y).context("y coordinate out of range")?;
    let sec1 = [&[0x04][..], &x, &y].concat();
    PublicKey::from_sec1_bytes(&sec1).map_err(|_| anyhow!("public key is not a point on P-256"))
}

/// Minimal two's-complement big-endian encoding of a non-negative number, as
/// Java's `BigInteger.toByteArray()` produces it: no leading zero bytes, but a
/// `0x00` in front when the top bit would otherwise be set.
fn signed_be(n: &[u8; 32]) -> Vec<u8> {
    let start = n.iter().position(|&b| b != 0).unwrap_or(n.len() - 1);
    let digits = &n[start..];
    let mut out = Vec::with_capacity(digits.len() + 1);
    if digits[0] & 0x80 != 0 {
        out.push(0);
    }
    out.extend_from_slice(digits);
    out
}

/// Reads a big-endian coordinate of any length into 32 bytes; `None` if the
/// value does not fit.
fn unsigned_32(b: &[u8]) -> Option<[u8; 32]> {
    let start = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    let digits = &b[start..];
    let mut out = [0u8; 32];
    out.get_mut(32usize.checked_sub(digits.len())?..)?.copy_from_slice(digits);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::thread;

    fn client_finish_for(secret: &EphemeralSecret) -> Vec<u8> {
        let finish = Ukey2ClientFinished {
            public_key: Some(encode_public_key(&secret.public_key())),
        };
        message(Type::ClientFinish, finish.encode_to_vec())
    }

    fn client_init(version: i32, commitment: Vec<u8>) -> Vec<u8> {
        let init = Ukey2ClientInit {
            version: Some(version),
            random: Some(vec![7; 32]),
            cipher_commitments: vec![ukey2_client_init::CipherCommitment {
                handshake_cipher: Some(Ukey2HandshakeCipher::P256Sha512 as i32),
                commitment: Some(commitment),
            }],
            next_protocol: Some("AES_256_CBC-HMAC_SHA256".into()),
        };
        message(Type::ClientInit, init.encode_to_vec())
    }

    #[test]
    fn pin_vectors() {
        let seq: [u8; 32] = core::array::from_fn(|i| i as u8);
        assert_eq!(pin(&seq), "5095");
        assert_eq!(pin(&[0xff; 32]), "6509");
        let mut k = [0u8; 32];
        k[0] = 0x80;
        assert_eq!(pin(&k), "0128");
    }

    #[test]
    fn handshake_server_and_client_agree() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let srv = thread::spawn(move || {
            let mut w = a.try_clone().unwrap();
            server(&mut a, &mut w)
        });
        let mut w = b.try_clone().unwrap();
        let c = client(&mut b, &mut w).unwrap();
        let s = srv.join().unwrap().unwrap();
        assert_eq!(s.auth, c.auth);
        assert_eq!(s.encrypt, c.decrypt);
        assert_eq!(s.decrypt, c.encrypt);
        assert_eq!(s.send_hmac, c.recv_hmac);
        assert_eq!(s.recv_hmac, c.send_hmac);
        assert_ne!(s.encrypt, s.decrypt);
        assert_ne!(s.send_hmac, s.recv_hmac);
        assert_eq!(pin(&s.auth), pin(&c.auth));
    }

    #[test]
    fn server_rejects_wrong_commitment() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let srv = thread::spawn(move || {
            let mut w = a.try_clone().unwrap();
            server(&mut a, &mut w)
        });
        let committed = EphemeralSecret::generate_from_rng(&mut rand::rng());
        let sent = EphemeralSecret::generate_from_rng(&mut rand::rng());
        let commitment = Sha512::digest(client_finish_for(&committed)).to_vec();
        write_frame(&mut b, &client_init(1, commitment)).unwrap();
        let server_init = Ukey2Message::decode(read_frame(&mut b).unwrap().as_slice()).unwrap();
        assert_eq!(server_init.message_type, Some(Type::ServerInit as i32));
        write_frame(&mut b, &client_finish_for(&sent)).unwrap();
        assert!(srv.join().unwrap().is_err());
    }

    #[test]
    fn server_alerts_on_bad_version() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let srv = thread::spawn(move || {
            let mut w = a.try_clone().unwrap();
            server(&mut a, &mut w)
        });
        write_frame(&mut b, &client_init(2, vec![0; 64])).unwrap();
        let msg = Ukey2Message::decode(read_frame(&mut b).unwrap().as_slice()).unwrap();
        assert_eq!(msg.message_type, Some(Type::Alert as i32));
        let alert = Ukey2Alert::decode(msg.message_data.unwrap().as_slice()).unwrap();
        assert_eq!(alert.r#type, Some(AlertType::BadVersion as i32));
        assert!(srv.join().unwrap().is_err());
    }

    #[test]
    fn signed_coordinate_encoding() {
        let mut high = [0x11u8; 32];
        high[0] = 0x80;
        let enc = signed_be(&high);
        assert_eq!(enc.len(), 33);
        assert_eq!(enc[0], 0x00);
        assert_eq!(&enc[1..], &high[..]);
        assert_eq!(unsigned_32(&enc), Some(high));

        let low = [0x7fu8; 32];
        assert_eq!(signed_be(&low), low.to_vec());
        assert_eq!(unsigned_32(&low), Some(low));

        // Java's BigInteger.toByteArray() drops leading zero bytes.
        let mut short = [0x22u8; 32];
        short[0] = 0;
        assert_eq!(signed_be(&short), short[1..].to_vec());
        assert_eq!(unsigned_32(&short[1..]), Some(short));

        assert_eq!(unsigned_32(&[1u8; 33]), None);
    }

    #[test]
    fn d2d_salt_is_sha256_of_d2d() {
        // The constant NearDrop hard-codes for the D2D salt.
        let expected: [u8; 32] = [
            0x82, 0xAA, 0x55, 0xA0, 0xD3, 0x97, 0xF8, 0x83, 0x46, 0xCA, 0x1C, 0xEE, 0x8D, 0x39, 0x09, 0xB9, 0x5F, 0x13, 0xFA, 0x7D, 0xEB, 0x1D, 0x4A, 0xB3,
            0x83, 0x76, 0xB8, 0x25, 0x6D, 0xA8, 0x55, 0x10,
        ];
        assert_eq!(<[u8; 32]>::from(Sha256::digest(b"D2D")), expected);
    }

    #[test]
    fn derive_orients_keys_by_role() {
        let x = [5u8; 32];
        let s = derive(&x, b"init", b"server", Role::Server);
        let c = derive(&x, b"init", b"server", Role::Client);
        assert_eq!(s.auth, c.auth);
        assert_eq!((s.encrypt, s.send_hmac), (c.decrypt, c.recv_hmac));
        assert_eq!((s.decrypt, s.recv_hmac), (c.encrypt, c.send_hmac));
        // The info covers both handshake messages in order.
        assert_ne!(derive(&x, b"server", b"init", Role::Server).auth, s.auth);
    }
}
