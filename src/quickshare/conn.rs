//! The connection after the plaintext part (ConnectionRequest, UKEY2,
//! ConnectionResponse): encrypted `OfflineFrame`s, the payload layer on top of
//! them (BYTES payloads carrying `sharing.nearby.Frame`s, FILE payloads in
//! chunks) and the keep-alive Android expects every 10 seconds.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use prost::Message;

use super::frame::{MAX_FRAME, read_frame, write_frame};
use super::proto;
use super::proto::connections::payload_transfer_frame::payload_chunk::Flags;
use super::proto::connections::payload_transfer_frame::payload_header::PayloadType;
use super::proto::connections::payload_transfer_frame::{PacketType, PayloadChunk, PayloadHeader};
use super::proto::connections::v1_frame::FrameType;
use super::proto::connections::{DisconnectionFrame, KeepAliveFrame, OfflineFrame, PayloadTransferFrame, V1Frame, offline_frame};
use super::secure::SecureChannel;
use super::ukey2::Keys;

/// How long the connection may stay silent before a `KEEP_ALIVE` goes out.
#[cfg(not(test))]
pub const KEEP_ALIVE: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const KEEP_ALIVE: Duration = Duration::from_millis(200);

/// Frames the reader thread may read ahead of [`Conn::recv`]. Bounded, so a
/// fast sender waits for the disk instead of filling memory.
const READ_AHEAD: usize = 4;

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
    /// The peer sent a DISCONNECTION frame or closed the connection.
    Disconnected,
}

/// An encrypted connection. A reader thread reads frames off the link; the
/// owner sends and receives through [`Conn`], which also keeps the connection
/// alive while the owner waits.
pub struct Conn {
    link: Box<dyn Link>,
    secure: SecureChannel,
    incoming: mpsc::Receiver<io::Result<Vec<u8>>>,
    last_send: Instant,
    next_payload_id: i64,
    /// BYTES payloads still waiting for their last chunk, by payload id.
    bytes: HashMap<i64, Vec<u8>>,
    closed: bool,
}

impl Conn {
    /// Starts the encrypted part of a connection. Call it after the plaintext
    /// ConnectionRequest, UKEY2 and ConnectionResponse.
    pub fn new(link: Box<dyn Link>, keys: &Keys) -> io::Result<Conn> {
        let mut reader = link.reader()?;
        let (tx, incoming) = mpsc::sync_channel(READ_AHEAD);
        // Ends on the first read error (EOF after `close`) or once the
        // receiver is gone.
        thread::Builder::new().name("quickshare-conn".into()).spawn(move || {
            loop {
                let frame = read_frame(&mut reader);
                let failed = frame.is_err();
                if tx.send(frame).is_err() || failed {
                    break;
                }
            }
        })?;
        Ok(Conn {
            link,
            secure: SecureChannel::new(keys),
            incoming,
            last_send: Instant::now(),
            // Payload ids only need to be unique within the connection; file
            // payload ids come from the Introduction, so start somewhere random.
            next_payload_id: rand::random::<i64>() & (i64::MAX >> 1),
            bytes: HashMap::new(),
            closed: false,
        })
    }

    /// Sends a `sharing.nearby.Frame` as a BYTES payload the way Android does:
    /// one chunk with the whole message, then an empty one with `LAST_CHUNK`.
    pub fn send_sharing(&mut self, f: &proto::sharing::Frame) -> anyhow::Result<()> {
        let id = self.next_payload_id;
        self.next_payload_id = self.next_payload_id.wrapping_add(1);
        let body = f.encode_to_vec();
        let total = i64::try_from(body.len())?;
        self.send_payload(id, PayloadType::Bytes, total, 0, Some(body), false)?;
        self.send_payload(id, PayloadType::Bytes, total, total, None, true)?;
        Ok(())
    }

    /// Sends one chunk of a FILE payload.
    pub fn send_file_chunk(&mut self, payload_id: i64, total: i64, offset: i64, body: &[u8], last: bool) -> anyhow::Result<()> {
        let body = (!body.is_empty()).then(|| body.to_vec());
        self.send_payload(payload_id, PayloadType::File, total, offset, body, last)?;
        Ok(())
    }

    /// Tells the peer the connection is over.
    pub fn send_disconnect(&mut self) -> anyhow::Result<()> {
        self.send(V1Frame {
            r#type: Some(FrameType::Disconnection as i32),
            disconnection: Some(DisconnectionFrame::default()),
            ..V1Frame::default()
        })?;
        Ok(())
    }

    /// Waits at most `timeout` for the next payload or the end of the
    /// connection (`Ok(None)` on timeout). Meanwhile it sends a `KEEP_ALIVE`
    /// whenever nothing went out for [`KEEP_ALIVE`], answers the peer's
    /// keep-alives and collects BYTES payloads until their last chunk.
    pub fn recv(&mut self, timeout: Duration) -> anyhow::Result<Option<Incoming>> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.closed {
                return Ok(Some(Incoming::Disconnected));
            }
            if self.last_send.elapsed() >= KEEP_ALIVE
                && let Err(e) = self.send_keep_alive(false)
            {
                return self.failed(e);
            }
            let until_keep_alive = KEEP_ALIVE.saturating_sub(self.last_send.elapsed());
            let wait = deadline.saturating_duration_since(Instant::now()).min(until_keep_alive);
            match self.incoming.recv_timeout(wait) {
                Ok(Ok(frame)) => {
                    if let Some(incoming) = self.handle(&frame)? {
                        return Ok(Some(incoming));
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
        let plain = self.secure.open(frame)?;
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
            _ => Ok(None),
        }
    }

    fn payload(&mut self, transfer: PayloadTransferFrame) -> anyhow::Result<Option<Incoming>> {
        if matches!(transfer.packet_type(), PacketType::Control | PacketType::PayloadAck) {
            return Ok(None);
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

    fn send(&mut self, v1: V1Frame) -> io::Result<()> {
        let frame = OfflineFrame {
            version: Some(offline_frame::Version::V1 as i32),
            v1: Some(v1),
        };
        let sealed = self.secure.seal(&frame.encode_to_vec());
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

impl Drop for Conn {
    fn drop(&mut self) {
        // Wakes the reader thread; it exits on the resulting read error.
        let _ = self.link.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
