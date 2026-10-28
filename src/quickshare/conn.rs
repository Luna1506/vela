//! The connection after the plaintext part (ConnectionRequest, UKEY2,
//! ConnectionResponse): encrypted `OfflineFrame`s, the payload layer on top of
//! them (BYTES payloads carrying `sharing.nearby.Frame`s, FILE payloads in
//! chunks), the keep-alive Android expects every 10 seconds and the move to
//! another link in a bandwidth upgrade ([`Conn::switch_to`]).

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use prost::Message;

use super::frame::{MAX_FRAME, read_frame, write_frame};
use super::proto::connections::bandwidth_upgrade_negotiation_frame::EventType as UpgradeEvent;
use super::proto::connections::payload_transfer_frame::control_message::EventType;
use super::proto::connections::payload_transfer_frame::payload_chunk::Flags;
use super::proto::connections::payload_transfer_frame::payload_header::PayloadType;
use super::proto::connections::payload_transfer_frame::{PacketType, PayloadChunk, PayloadHeader};
use super::proto::connections::v1_frame::FrameType;
use super::proto::connections::{
    BandwidthUpgradeNegotiationFrame, DisconnectionFrame, KeepAliveFrame, OfflineFrame, PayloadTransferFrame, V1Frame, offline_frame,
};
use super::secure::SecureChannel;
use super::ukey2::Keys;
use super::{bwu, proto, write_plain};

/// How long the connection may stay silent before a `KEEP_ALIVE` goes out.
#[cfg(not(test))]
pub const KEEP_ALIVE: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const KEEP_ALIVE: Duration = Duration::from_millis(200);

/// Frames the reader thread may read ahead of [`Conn::recv`]. Bounded, so a
/// fast sender waits for the disk instead of filling memory.
const READ_AHEAD: usize = 4;

/// Upper bound on what sealing adds to an `OfflineFrame` (sequence number,
/// padding, header with IV, signature, protobuf tags). Checked before
/// sealing, so a frame too large to send never uses up a sequence number.
const SEAL_OVERHEAD: usize = 256;

/// Stands in for "no deadline" when `now + timeout` does not fit an `Instant`.
const FOREVER: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// A transport the connection runs over. Wi-Fi LAN uses a `TcpStream`; another
/// medium only needs its own `Link`.
pub trait Link: Read + Write + Send + 'static {
    /// A second handle for reading, used by the reader thread.
    fn reader(&self) -> io::Result<Box<dyn Read + Send>>;
    /// Closes both directions, so a read blocked on [`Link::reader`] returns.
    fn close(&self) -> io::Result<()>;
}

impl Link for TcpStream {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn close(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Both)
    }
}

/// What [`Conn::recv`] hands up. Each value is matched once and dropped, so
/// the size of the `Sharing` variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// A complete BYTES payload, decoded.
    Sharing(proto::sharing::Frame),
    /// One chunk of a FILE payload, passed on as it arrives.
    FileChunk { payload_id: i64, offset: i64, body: Vec<u8>, last: bool },
    /// A complete BYTES payload announced with [`Conn::expect_bytes`]
    /// (text, for example), passed on as it is.
    Bytes { payload_id: i64, data: Vec<u8> },
    /// The peer cancelled a payload or gave up on it (CONTROL packet with
    /// PAYLOAD_CANCELED or PAYLOAD_ERROR).
    PayloadCancelled { payload_id: i64 },
    /// The peer sent a DISCONNECTION frame or closed the connection.
    Disconnected,
    /// A bandwidth upgrade frame other than the two the switch itself uses
    /// (see [`Conn::switch_to`]).
    Upgrade(BandwidthUpgradeNegotiationFrame),
}

/// Frames read off a link by its reader thread.
type Frames = mpsc::Receiver<io::Result<Vec<u8>>>;

/// The link a connection is moving to.
struct Switch {
    link: Box<dyn Link>,
    incoming: Frames,
    /// SAFE_TO_CLOSE_PRIOR_CHANNEL went out on the old link.
    safe_sent: bool,
}

/// An encrypted connection. A reader thread reads frames off the link; the
/// owner sends and receives through [`Conn`], which also keeps the connection
/// alive while the owner waits.
pub struct Conn {
    link: Box<dyn Link>,
    secure: SecureChannel,
    incoming: Frames,
    last_send: Instant,
    next_payload_id: i64,
    /// BYTES payloads still waiting for their last chunk, by payload id.
    bytes: HashMap<i64, Vec<u8>>,
    /// BYTES payload ids handed up raw instead of decoded as sharing frames.
    raw: HashSet<i64>,
    closed: bool,
    switch: Option<Switch>,
    /// The peer's LAST_WRITE_TO_PRIOR_CHANNEL came: it writes nothing more
    /// on `link`.
    peer_done: bool,
    /// What to send once the switch is done. Sealed only then, so that the
    /// sequence numbers stay in the order the peer reads them.
    queued: Vec<V1Frame>,
}

/// Reads frames off `link` on a thread of its own, at most [`READ_AHEAD`]
/// ahead. The thread ends on the first read error (EOF after `close`) or
/// once the receiver is gone.
fn read_frames(link: &dyn Link) -> io::Result<Frames> {
    let mut reader = link.reader()?;
    let (tx, incoming) = mpsc::sync_channel(READ_AHEAD);
    thread::Builder::new().name("quickshare-conn".into()).spawn(move || {
        loop {
            let frame = read_frame(&mut reader);
            let failed = frame.is_err();
            if tx.send(frame).is_err() || failed {
                break;
            }
        }
    })?;
    Ok(incoming)
}

impl Conn {
    /// Starts the encrypted part of a connection. Call it after the plaintext
    /// ConnectionRequest, UKEY2 and ConnectionResponse.
    pub fn new(link: Box<dyn Link>, keys: &Keys) -> io::Result<Conn> {
        let incoming = read_frames(link.as_ref())?;
        Ok(Conn {
            link,
            secure: SecureChannel::new(keys),
            incoming,
            last_send: Instant::now(),
            // Payload ids only need to be unique within the connection; file
            // payload ids come from the Introduction, so start somewhere random.
            next_payload_id: rand::random::<i64>() & (i64::MAX >> 1),
            bytes: HashMap::new(),
            raw: HashSet::new(),
            closed: false,
            switch: None,
            peer_done: false,
            queued: Vec::new(),
        })
    }

    /// Sends a bandwidth upgrade frame.
    pub fn send_upgrade(&mut self, f: BandwidthUpgradeNegotiationFrame) -> anyhow::Result<()> {
        self.send(bwu::v1(f))?;
        Ok(())
    }

    /// Moves the connection to `link` (a bandwidth upgrade; the plaintext
    /// CLIENT_INTRODUCTION and its ack have already gone over it), keeping
    /// the keys and the sequence numbers. LAST_WRITE_TO_PRIOR_CHANNEL goes
    /// out on the old link now and SAFE_TO_CLOSE_PRIOR_CHANNEL once the
    /// peer's LAST_WRITE came; everything else waits. Once the peer's
    /// SAFE_TO_CLOSE came too (or the old link ended), the old link gets an
    /// unencrypted DISCONNECTION and is closed, and the connection goes on
    /// over `link`. Frames from `link` are decrypted only after everything
    /// read from the old one.
    pub fn switch_to(&mut self, link: Box<dyn Link>) -> anyhow::Result<()> {
        if self.switch.is_some() {
            bail!("the connection is already moving to another link");
        }
        let incoming = read_frames(link.as_ref())?;
        self.write_now(bwu::v1(bwu::event(UpgradeEvent::LastWriteToPriorChannel)))?;
        self.switch = Some(Switch {
            link,
            incoming,
            safe_sent: false,
        });
        if self.peer_done {
            self.send_safe_to_close()?;
        }
        Ok(())
    }

    /// Between [`Conn::switch_to`] and the end of the switch.
    pub fn switching(&self) -> bool {
        self.switch.is_some()
    }

    /// Sends a `sharing.nearby.Frame` as a BYTES payload the way Android does:
    /// one chunk with the whole message, then an empty one with `LAST_CHUNK`.
    pub fn send_sharing(&mut self, f: &proto::sharing::Frame) -> anyhow::Result<()> {
        let id = self.next_payload_id;
        self.next_payload_id = self.next_payload_id.wrapping_add(1);
        self.send_bytes(id, &f.encode_to_vec())
    }

    /// Sends `data` as the BYTES payload `payload_id`, in the same two chunks
    /// as [`Conn::send_sharing`].
    pub fn send_bytes(&mut self, payload_id: i64, data: &[u8]) -> anyhow::Result<()> {
        let total = i64::try_from(data.len())?;
        self.send_payload(payload_id, PayloadType::Bytes, total, 0, Some(data.to_vec()), false)?;
        self.send_payload(payload_id, PayloadType::Bytes, total, total, None, true)?;
        Ok(())
    }

    /// Hands the BYTES payload `payload_id` up as [`Incoming::Bytes`] instead
    /// of decoding it as a sharing frame.
    pub fn expect_bytes(&mut self, payload_id: i64) {
        self.raw.insert(payload_id);
    }

    /// Sends one chunk of a FILE payload.
    pub fn send_file_chunk(&mut self, payload_id: i64, total: i64, offset: i64, body: &[u8], last: bool) -> anyhow::Result<()> {
        let body = (!body.is_empty()).then(|| body.to_vec());
        self.send_payload(payload_id, PayloadType::File, total, offset, body, last)?;
        Ok(())
    }

    /// Tells the peer the connection is over.
    pub fn send_disconnect(&mut self) -> anyhow::Result<()> {
        self.send(disconnection())?;
        Ok(())
    }

    /// Waits at most `timeout` for the next payload or the end of the
    /// connection (`Ok(None)` on timeout). Meanwhile it sends a `KEEP_ALIVE`
    /// whenever nothing went out for [`KEEP_ALIVE`], answers the peer's
    /// keep-alives and collects BYTES payloads until their last chunk. An
    /// error ends the connection: later calls return [`Incoming::Disconnected`].
    pub fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<Incoming>> {
        let result = self.recv_until(timeout);
        if result.is_err() {
            self.closed = true;
        }
        result
    }

    fn recv_until(&mut self, timeout: Duration) -> anyhow::Result<Option<Incoming>> {
        let now = Instant::now();
        let deadline = now.checked_add(timeout).unwrap_or_else(|| now + FOREVER);
        loop {
            if self.closed {
                return Ok(Some(Incoming::Disconnected));
            }
            // While switching, keep-alives wait for the new link.
            if !self.switching()
                && self.last_send.elapsed() >= KEEP_ALIVE
                && let Err(e) = self.send_keep_alive(false)
            {
                return self.failed(e);
            }
            let until_keep_alive = if self.switching() {
                KEEP_ALIVE
            } else {
                KEEP_ALIVE.saturating_sub(self.last_send.elapsed())
            };
            let wait = deadline.saturating_duration_since(Instant::now()).min(until_keep_alive);
            match self.incoming.recv_timeout(wait) {
                Ok(Ok(frame)) => {
                    if let Some(incoming) = self.handle(&frame)? {
                        return Ok(Some(incoming));
                    }
                    // Swallowed (keep-alive, partial BYTES payload, …): a
                    // steady stream of those must not hold `recv` past its time.
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                // The old link ended before the peer's SAFE_TO_CLOSE.
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) if self.switching() => {
                    if let Err(e) = self.finish_switch(false) {
                        return self.failed(e);
                    }
                }
                Ok(Err(e)) => return self.failed(e),
                Err(RecvTimeoutError::Disconnected) => self.closed = true,
                Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => return Ok(None),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn handle(&mut self, frame: &[u8]) -> anyhow::Result<Option<Incoming>> {
        let plain = match self.secure.open(frame) {
            Ok(plain) => plain,
            // The peer's last word on the old link is not encrypted.
            Err(_) if self.switching() && is_plain_disconnection(frame) => {
                if let Err(e) = self.finish_switch(true) {
                    return self.failed(e);
                }
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let frame = OfflineFrame::decode(plain.as_slice()).context("decoding OfflineFrame")?;
        let Some(v1) = frame.v1 else {
            return Ok(None);
        };
        match v1.r#type() {
            FrameType::PayloadTransfer => self.payload(v1.payload_transfer.context("PAYLOAD_TRANSFER without payload")?),
            FrameType::KeepAlive => {
                if !v1.keep_alive.is_some_and(|k| k.ack())
                    && let Err(e) = self.send_keep_alive(true)
                {
                    return self.failed(e);
                }
                Ok(None)
            }
            FrameType::Disconnection => {
                self.closed = true;
                Ok(Some(Incoming::Disconnected))
            }
            FrameType::BandwidthUpgradeNegotiation => {
                let f = v1.bandwidth_upgrade_negotiation.context("BANDWIDTH_UPGRADE_NEGOTIATION without content")?;
                let done = match f.event_type() {
                    UpgradeEvent::LastWriteToPriorChannel => {
                        self.peer_done = true;
                        match &self.switch {
                            Some(s) if !s.safe_sent => self.send_safe_to_close(),
                            _ => Ok(()),
                        }
                    }
                    UpgradeEvent::SafeToClosePriorChannel if self.switching() => self.finish_switch(true),
                    _ => return Ok(Some(Incoming::Upgrade(f))),
                };
                match done {
                    Ok(()) => Ok(None),
                    Err(e) => self.failed(e),
                }
            }
            _ => Ok(None),
        }
    }

    fn send_safe_to_close(&mut self) -> io::Result<()> {
        let safe = bwu::BandwidthUpgradeNegotiationFrame {
            safe_to_close_prior_channel: Some(Default::default()),
            ..bwu::event(UpgradeEvent::SafeToClosePriorChannel)
        };
        self.write_now(bwu::v1(safe))?;
        if let Some(s) = &mut self.switch {
            s.safe_sent = true;
        }
        Ok(())
    }

    /// Ends the switch: from now on the new link carries the connection.
    /// `old_alive`: the old link still takes the last frames.
    fn finish_switch(&mut self, old_alive: bool) -> io::Result<()> {
        let Some(switch) = self.switch.take() else {
            return Ok(());
        };
        if old_alive {
            if !switch.safe_sent {
                let _ = self.send_safe_to_close();
            }
            let _ = write_plain(&mut self.link, disconnection());
        }
        let _ = self.link.close();
        self.link = switch.link;
        // Whatever the peer still sent on the old link is not read.
        self.incoming = switch.incoming;
        self.peer_done = false;
        for v1 in std::mem::take(&mut self.queued) {
            self.write_now(v1)?;
        }
        Ok(())
    }

    fn payload(&mut self, transfer: PayloadTransferFrame) -> anyhow::Result<Option<Incoming>> {
        match transfer.packet_type() {
            PacketType::PayloadAck => return Ok(None),
            PacketType::Control => return Ok(self.control(&transfer)),
            _ => {}
        }
        let header = transfer.payload_header.context("payload without header")?;
        let chunk = transfer.payload_chunk.context("payload without chunk")?;
        let id = header.id.context("payload header without id")?;
        let last = chunk.flags() & Flags::LastChunk as i32 != 0;
        let offset = chunk.offset();
        let body = chunk.body.unwrap_or_default();
        match header.r#type() {
            PayloadType::File => Ok(Some(Incoming::FileChunk {
                payload_id: id,
                offset,
                body,
                last,
            })),
            PayloadType::Bytes => self.bytes_chunk(id, header.total_size(), offset, &body, last),
            other => bail!("unsupported payload type {}", other.as_str_name()),
        }
    }

    fn control(&mut self, transfer: &PayloadTransferFrame) -> Option<Incoming> {
        let payload_id = transfer.payload_header.as_ref()?.id?;
        match transfer.control_message?.event() {
            EventType::PayloadCanceled | EventType::PayloadError => {
                self.bytes.remove(&payload_id);
                Some(Incoming::PayloadCancelled { payload_id })
            }
            _ => None,
        }
    }

    fn bytes_chunk(&mut self, id: i64, total: i64, offset: i64, body: &[u8], last: bool) -> anyhow::Result<Option<Incoming>> {
        let total = usize::try_from(total).ok().filter(|&t| t <= MAX_FRAME);
        let Some(total) = total else {
            self.bytes.remove(&id);
            bail!("BYTES payload {id} exceeds the {MAX_FRAME} byte limit");
        };
        // All unfinished BYTES payloads together stay within one frame's size.
        let buffered: usize = self.bytes.values().map(Vec::len).sum();
        let buf = self.bytes.entry(id).or_default();
        if usize::try_from(offset).ok() != Some(buf.len()) {
            bail!("BYTES payload {id}: chunk at offset {offset}, expected {}", buf.len());
        }
        if buf.len() + body.len() > total || buffered + body.len() > MAX_FRAME {
            bail!("BYTES payload {id} is larger than announced or than {MAX_FRAME} bytes");
        }
        buf.extend_from_slice(body);
        if !last {
            return Ok(None);
        }
        let data = self.bytes.remove(&id).unwrap_or_default();
        if self.raw.remove(&id) {
            return Ok(Some(Incoming::Bytes { payload_id: id, data }));
        }
        let frame = proto::sharing::Frame::decode(data.as_slice()).context("decoding sharing frame")?;
        Ok(Some(Incoming::Sharing(frame)))
    }

    /// A closed connection ends as [`Incoming::Disconnected`]; anything else
    /// is an error.
    fn failed(&mut self, e: io::Error) -> anyhow::Result<Option<Incoming>> {
        self.closed = true;
        match e.kind() {
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::BrokenPipe => {
                Ok(Some(Incoming::Disconnected))
            }
            _ => Err(anyhow::Error::new(e).context("Quick Share connection failed")),
        }
    }

    fn send_keep_alive(&mut self, ack: bool) -> io::Result<()> {
        self.send(V1Frame {
            r#type: Some(FrameType::KeepAlive as i32),
            keep_alive: Some(KeepAliveFrame { ack: Some(ack), seq_num: None }),
            ..V1Frame::default()
        })
    }

    fn send_payload(&mut self, id: i64, kind: PayloadType, total: i64, offset: i64, body: Option<Vec<u8>>, last: bool) -> io::Result<()> {
        self.send(V1Frame {
            r#type: Some(FrameType::PayloadTransfer as i32),
            payload_transfer: Some(PayloadTransferFrame {
                packet_type: Some(PacketType::Data as i32),
                payload_header: Some(PayloadHeader {
                    id: Some(id),
                    r#type: Some(kind as i32),
                    total_size: Some(total),
                    is_sensitive: Some(false),
                    ..PayloadHeader::default()
                }),
                payload_chunk: Some(PayloadChunk {
                    flags: Some(if last { Flags::LastChunk as i32 } else { 0 }),
                    offset: Some(offset),
                    body,
                    index: None,
                }),
                control_message: None,
            }),
            ..V1Frame::default()
        })
    }

    /// Sends `v1`, or keeps it for the new link while switching (a
    /// keep-alive is dropped then).
    fn send(&mut self, v1: V1Frame) -> io::Result<()> {
        if self.switching() {
            if v1.r#type() != FrameType::KeepAlive {
                self.queued.push(v1);
            }
            return Ok(());
        }
        self.write_now(v1)
    }

    fn write_now(&mut self, v1: V1Frame) -> io::Result<()> {
        let frame = OfflineFrame {
            version: Some(offline_frame::Version::V1 as i32),
            v1: Some(v1),
        };
        let plain = frame.encode_to_vec();
        if plain.len() + SEAL_OVERHEAD > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large to send"));
        }
        let sealed = self.secure.seal(&plain);
        // Length prefix and message in one write, so Nagle does not hold the
        // message back behind the 4-byte prefix.
        let mut buf = Vec::with_capacity(4 + sealed.len());
        write_frame(&mut buf, &sealed)?;
        self.link.write_all(&buf)?;
        self.link.flush()?;
        self.last_send = Instant::now();
        Ok(())
    }
}

fn disconnection() -> V1Frame {
    V1Frame {
        r#type: Some(FrameType::Disconnection as i32),
        disconnection: Some(DisconnectionFrame::default()),
        ..V1Frame::default()
    }
}

/// An unencrypted `OfflineFrame` with a DISCONNECTION.
fn is_plain_disconnection(frame: &[u8]) -> bool {
    OfflineFrame::decode(frame).is_ok_and(|f| f.v1.is_some_and(|v1| v1.r#type() == FrameType::Disconnection))
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Wakes the reader threads; they exit on the resulting read error.
        let _ = self.link.close();
        if let Some(s) = &self.switch {
            let _ = s.link.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quickshare::proto::connections::payload_transfer_frame::{ControlMessage, control_message};
    use crate::quickshare::proto::sharing;
    use crate::quickshare::ukey2;
    use std::net::TcpListener;

    const WAIT: Duration = Duration::from_secs(2);

    /// Two ends of a TCP connection with keys from a real UKEY2 handshake.
    fn handshake() -> ((TcpStream, Keys), (TcpStream, Keys)) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let keys = ukey2::server(&mut s, &mut w).unwrap();
            (s, keys)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let mut w = c.try_clone().unwrap();
        let keys = ukey2::client(&mut c, &mut w).unwrap();
        (server.join().unwrap(), (c, keys))
    }

    fn pair() -> (Conn, Conn) {
        let ((s, sk), (c, ck)) = handshake();
        (Conn::new(Box::new(s), &sk).unwrap(), Conn::new(Box::new(c), &ck).unwrap())
    }

    /// A `Conn` and a raw peer that reads and writes frames by hand.
    fn raw_pair() -> (Conn, TcpStream, SecureChannel) {
        let ((s, sk), (c, ck)) = handshake();
        c.set_read_timeout(Some(WAIT)).unwrap();
        (Conn::new(Box::new(s), &sk).unwrap(), c, SecureChannel::new(&ck))
    }

    fn send_raw(s: &mut TcpStream, chan: &mut SecureChannel, v1: V1Frame) {
        let frame = OfflineFrame {
            version: Some(offline_frame::Version::V1 as i32),
            v1: Some(v1),
        };
        write_frame(s, &chan.seal(&frame.encode_to_vec())).unwrap();
    }

    fn recv_raw(s: &mut TcpStream, chan: &mut SecureChannel) -> V1Frame {
        let plain = chan.open(&read_frame(s).unwrap()).unwrap();
        OfflineFrame::decode(plain.as_slice()).unwrap().v1.unwrap()
    }

    /// The next frame from the `Conn` that is not a keep-alive.
    fn recv_raw_skipping_keep_alive(s: &mut TcpStream, chan: &mut SecureChannel) -> V1Frame {
        loop {
            let v1 = recv_raw(s, chan);
            if v1.r#type() != FrameType::KeepAlive {
                return v1;
            }
        }
    }

    fn bytes_payload(id: i64, total: i64, offset: i64, body: Vec<u8>, last: bool) -> V1Frame {
        V1Frame {
            r#type: Some(FrameType::PayloadTransfer as i32),
            payload_transfer: Some(PayloadTransferFrame {
                packet_type: Some(PacketType::Data as i32),
                payload_header: Some(PayloadHeader {
                    id: Some(id),
                    r#type: Some(PayloadType::Bytes as i32),
                    total_size: Some(total),
                    ..Default::default()
                }),
                payload_chunk: Some(PayloadChunk {
                    flags: Some(i32::from(last)),
                    offset: Some(offset),
                    body: Some(body),
                    index: None,
                }),
                control_message: None,
            }),
            ..Default::default()
        }
    }

    fn introduction() -> sharing::Frame {
        sharing::Frame {
            version: Some(sharing::frame::Version::V1 as i32),
            v1: Some(sharing::V1Frame {
                r#type: Some(sharing::v1_frame::FrameType::Introduction as i32),
                introduction: Some(sharing::IntroductionFrame {
                    file_metadata: vec![sharing::FileMetadata {
                        name: Some("photo.jpg".into()),
                        payload_id: Some(42),
                        size: Some(1234),
                        mime_type: Some("image/jpeg".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }
    }

    /// Waits up to [`WAIT`] for something other than a timeout.
    fn next(conn: &mut Conn) -> anyhow::Result<Incoming> {
        conn.recv(WAIT)?.ok_or_else(|| anyhow::anyhow!("nothing arrived within {WAIT:?}"))
    }

    #[test]
    fn sharing_frame_roundtrip() {
        let (mut a, mut b) = pair();
        a.send_sharing(&introduction()).unwrap();
        assert_eq!(next(&mut b).unwrap(), Incoming::Sharing(introduction()));
    }

    #[test]
    fn sharing_frame_is_one_bytes_chunk_then_an_empty_last_chunk() {
        // The two-frame pattern Android uses for every negotiation message.
        let (mut a, mut s, mut chan) = raw_pair();
        a.send_sharing(&introduction()).unwrap();
        let encoded = introduction().encode_to_vec();
        let len = encoded.len() as i64;

        let first = recv_raw_skipping_keep_alive(&mut s, &mut chan).payload_transfer.unwrap();
        assert_eq!(first.packet_type(), PacketType::Data);
        let header = first.payload_header.unwrap();
        assert_eq!(header.r#type(), PayloadType::Bytes);
        assert_eq!(header.total_size, Some(len));
        let chunk = first.payload_chunk.unwrap();
        assert_eq!((chunk.flags(), chunk.offset(), chunk.body.as_deref()), (0, 0, Some(encoded.as_slice())));

        let second = recv_raw_skipping_keep_alive(&mut s, &mut chan).payload_transfer.unwrap();
        assert_eq!(second.payload_header.unwrap().id, header.id);
        let chunk = second.payload_chunk.unwrap();
        assert_eq!(chunk.flags(), 1, "LAST_CHUNK");
        assert_eq!(chunk.offset(), len);
        assert!(chunk.body.unwrap_or_default().is_empty());
    }

    #[test]
    fn sharing_frames_get_distinct_payload_ids() {
        let (mut a, mut s, mut chan) = raw_pair();
        a.send_sharing(&introduction()).unwrap();
        a.send_sharing(&introduction()).unwrap();
        let ids: Vec<_> = (0..4)
            .map(|_| {
                recv_raw_skipping_keep_alive(&mut s, &mut chan)
                    .payload_transfer
                    .unwrap()
                    .payload_header
                    .unwrap()
                    .id
                    .unwrap()
            })
            .collect();
        assert_eq!(ids[0], ids[1]);
        assert_eq!(ids[2], ids[3]);
        assert_ne!(ids[0], ids[2]);
    }

    #[test]
    fn file_chunks_arrive_in_order() {
        let (mut a, mut b) = pair();
        let n = 1000;
        let chunks: Vec<Vec<u8>> = (0..3u8).map(|i| vec![i; n]).collect();
        for (i, body) in chunks.iter().enumerate() {
            a.send_file_chunk(42, 3 * n as i64, (i * n) as i64, body, i == 2).unwrap();
        }
        for (i, body) in chunks.into_iter().enumerate() {
            let expected = Incoming::FileChunk {
                payload_id: 42,
                offset: (i * n) as i64,
                body,
                last: i == 2,
            };
            assert_eq!(next(&mut b).unwrap(), expected);
        }
    }

    #[test]
    fn file_chunk_uses_the_file_payload_header() {
        let (mut a, mut s, mut chan) = raw_pair();
        a.send_file_chunk(42, 10, 4, b"abcdef", true).unwrap();
        let t = recv_raw_skipping_keep_alive(&mut s, &mut chan).payload_transfer.unwrap();
        let header = t.payload_header.unwrap();
        assert_eq!((header.id, header.r#type(), header.total_size), (Some(42), PayloadType::File, Some(10)));
        let chunk = t.payload_chunk.unwrap();
        assert_eq!((chunk.flags(), chunk.offset(), chunk.body.as_deref()), (1, 4, Some(&b"abcdef"[..])));
    }

    #[test]
    fn keepalive_while_waiting_for_decision() {
        let (mut a, mut s, mut chan) = raw_pair();
        let start = Instant::now();
        assert_eq!(a.recv(Duration::from_secs(1)).unwrap(), None);
        let waited = start.elapsed();
        assert!(waited >= Duration::from_secs(1), "returned early after {waited:?}");
        assert!(waited < Duration::from_millis(1500), "blocked for {waited:?}");
        for _ in 0..3 {
            let v1 = recv_raw(&mut s, &mut chan);
            assert_eq!(v1.r#type(), FrameType::KeepAlive);
            assert!(!v1.keep_alive.unwrap().ack());
        }
    }

    #[test]
    fn keepalive_also_goes_out_while_data_keeps_arriving() {
        // A long incoming file transfer: frames keep coming, nothing is sent
        // back, and the peer still has to see keep-alives.
        let (mut a, mut s, mut chan) = raw_pair();
        let sender = thread::spawn(move || {
            let start = Instant::now();
            let mut offset = 0;
            while start.elapsed() < Duration::from_millis(700) {
                let mut v1 = bytes_payload(1, 1000, offset, vec![0; 1], false);
                let t = v1.payload_transfer.as_mut().unwrap();
                t.payload_header.as_mut().unwrap().r#type = Some(PayloadType::File as i32);
                send_raw(&mut s, &mut chan, v1);
                offset += 1;
                thread::sleep(Duration::from_millis(20));
            }
            let mut keep_alives = 0;
            while let Ok(frame) = read_frame(&mut s) {
                let v1 = OfflineFrame::decode(chan.open(&frame).unwrap().as_slice()).unwrap().v1.unwrap();
                keep_alives += usize::from(v1.r#type() == FrameType::KeepAlive);
                if keep_alives == 3 {
                    break;
                }
            }
            keep_alives
        });
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            a.recv(Duration::from_millis(50)).unwrap();
        }
        assert_eq!(sender.join().unwrap(), 3);
    }

    #[test]
    fn keepalive_is_answered_with_an_ack() {
        let (mut a, mut s, mut chan) = raw_pair();
        let ping = V1Frame {
            r#type: Some(FrameType::KeepAlive as i32),
            keep_alive: Some(KeepAliveFrame {
                ack: Some(false),
                seq_num: None,
            }),
            ..Default::default()
        };
        send_raw(&mut s, &mut chan, ping);
        assert_eq!(a.recv(Duration::from_millis(100)).unwrap(), None);
        let reply = recv_raw(&mut s, &mut chan);
        assert_eq!(reply.r#type(), FrameType::KeepAlive);
        assert!(reply.keep_alive.unwrap().ack());
    }

    #[test]
    fn peer_close_is_disconnected() {
        let (mut a, s, _) = raw_pair();
        drop(s);
        assert_eq!(next(&mut a).unwrap(), Incoming::Disconnected);
        assert_eq!(a.recv(Duration::ZERO).unwrap(), Some(Incoming::Disconnected));
    }

    #[test]
    fn disconnection_frame_is_disconnected() {
        let (mut a, mut b) = pair();
        a.send_disconnect().unwrap();
        assert_eq!(next(&mut b).unwrap(), Incoming::Disconnected);
    }

    #[test]
    fn dropping_the_conn_closes_the_link() {
        let (a, mut s, _) = raw_pair();
        drop(a);
        let mut buf = [0u8; 64];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) => panic!("expected EOF, got {e}"),
            }
        }
    }

    #[test]
    fn oversized_bytes_payload_is_error() {
        let (mut a, mut s, mut chan) = raw_pair();
        send_raw(&mut s, &mut chan, bytes_payload(1, MAX_FRAME as i64 + 1, 0, vec![0; 16], false));
        assert!(next(&mut a).is_err());
    }

    #[test]
    fn bytes_payload_beyond_its_total_size_is_error() {
        let (mut a, mut s, mut chan) = raw_pair();
        send_raw(&mut s, &mut chan, bytes_payload(1, 4, 0, vec![0; 3], false));
        send_raw(&mut s, &mut chan, bytes_payload(1, 4, 3, vec![0; 3], true));
        assert!(next(&mut a).is_err());
    }

    #[test]
    fn bytes_payload_with_a_gap_is_error() {
        let (mut a, mut s, mut chan) = raw_pair();
        send_raw(&mut s, &mut chan, bytes_payload(1, 8, 0, vec![0; 2], false));
        send_raw(&mut s, &mut chan, bytes_payload(1, 8, 4, vec![0; 2], true));
        assert!(next(&mut a).is_err());
    }

    fn control_payload(id: i64, event: control_message::EventType) -> V1Frame {
        V1Frame {
            r#type: Some(FrameType::PayloadTransfer as i32),
            payload_transfer: Some(PayloadTransferFrame {
                packet_type: Some(PacketType::Control as i32),
                payload_header: Some(PayloadHeader {
                    id: Some(id),
                    r#type: Some(PayloadType::File as i32),
                    ..Default::default()
                }),
                payload_chunk: None,
                control_message: Some(ControlMessage {
                    event: Some(event as i32),
                    offset: Some(0),
                }),
            }),
            ..Default::default()
        }
    }

    /// Keys for a `Conn` (`peer == false`) and for the other end.
    fn fixed_keys(peer: bool) -> Keys {
        let (a, b) = if peer { ([2; 32], [1; 32]) } else { ([1; 32], [2; 32]) };
        let (sa, sb) = if peer { ([4; 32], [3; 32]) } else { ([3; 32], [4; 32]) };
        Keys {
            auth: [0; 32],
            encrypt: a,
            decrypt: b,
            send_hmac: sa,
            recv_hmac: sb,
        }
    }

    /// A link whose peer has already sent a long run of acknowledged
    /// keep-alives (which `recv` swallows) and then closes; writes are dropped.
    struct Flood;

    impl Flood {
        const FRAMES: usize = 2000;

        fn frames() -> Vec<u8> {
            let mut chan = SecureChannel::new(&fixed_keys(true));
            let frame = OfflineFrame {
                version: Some(offline_frame::Version::V1 as i32),
                v1: Some(V1Frame {
                    r#type: Some(FrameType::KeepAlive as i32),
                    keep_alive: Some(KeepAliveFrame {
                        ack: Some(true),
                        seq_num: None,
                    }),
                    ..Default::default()
                }),
            }
            .encode_to_vec();
            let mut out = Vec::new();
            for _ in 0..Self::FRAMES {
                write_frame(&mut out, &chan.seal(&frame)).unwrap();
            }
            out
        }
    }

    impl Read for Flood {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for Flood {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Link for Flood {
        fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(io::Cursor::new(Flood::frames())))
        }
        fn close(&self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn recv_keeps_its_timeout_while_swallowed_frames_keep_coming() {
        // The reader thread reads ahead far faster than `recv` decrypts, so
        // without a deadline check after each swallowed frame `recv` would
        // only return once the whole run is through.
        let mut conn = Conn::new(Box::new(Flood), &fixed_keys(false)).unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(conn.recv(Duration::ZERO).unwrap(), None);
    }

    #[test]
    fn recv_takes_a_huge_timeout() {
        let (mut a, mut b) = pair();
        a.send_sharing(&introduction()).unwrap();
        assert_eq!(b.recv(Duration::MAX).unwrap(), Some(Incoming::Sharing(introduction())));
    }

    #[test]
    fn an_error_ends_the_connection() {
        let (mut a, mut s, _) = raw_pair();
        write_frame(&mut s, b"not a secure message").unwrap();
        assert!(next(&mut a).is_err());
        let start = Instant::now();
        assert_eq!(a.recv(Duration::from_secs(1)).unwrap(), Some(Incoming::Disconnected));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn oversized_send_fails_without_breaking_the_sequence() {
        let (mut a, mut b) = pair();
        let body = vec![0; MAX_FRAME];
        assert!(a.send_file_chunk(1, MAX_FRAME as i64, 0, &body, false).is_err());
        a.send_sharing(&introduction()).unwrap();
        assert_eq!(next(&mut b).unwrap(), Incoming::Sharing(introduction()));
    }

    #[test]
    fn sealing_stays_within_the_overhead_bound() {
        let mut chan = SecureChannel::new(&fixed_keys(false));
        for len in [0, 1, 15, 16, 1000, 512 * 1024 + 100] {
            assert!(chan.seal(&vec![7; len]).len() <= len + SEAL_OVERHEAD, "{len}");
        }
    }

    #[test]
    fn payload_cancel_and_error_are_surfaced() {
        let (mut a, mut s, mut chan) = raw_pair();
        #[allow(deprecated)]
        send_raw(&mut s, &mut chan, control_payload(41, control_message::EventType::PayloadReceivedAck));
        send_raw(&mut s, &mut chan, control_payload(42, control_message::EventType::PayloadCanceled));
        send_raw(&mut s, &mut chan, control_payload(43, control_message::EventType::PayloadError));
        assert_eq!(next(&mut a).unwrap(), Incoming::PayloadCancelled { payload_id: 42 });
        assert_eq!(next(&mut a).unwrap(), Incoming::PayloadCancelled { payload_id: 43 });
    }

    #[test]
    fn expected_bytes_payload_arrives_raw() {
        let (mut a, mut b) = pair();
        b.expect_bytes(5);
        a.send_bytes(5, b"https://example.org").unwrap();
        let expected = Incoming::Bytes {
            payload_id: 5,
            data: b"https://example.org".to_vec(),
        };
        assert_eq!(next(&mut b).unwrap(), expected);
    }
}
