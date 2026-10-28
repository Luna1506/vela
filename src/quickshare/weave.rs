//! The Bluetooth LE data connection a phone opens to our GATT service
//! (layouts from google/nearby, Apache-2.0: `internal/weave/packet.{h,cc}`,
//! the Apple Weave peripheral `GNSPeripheralServiceManager.m`,
//! `GNCMBleConnection.m` and `proto/mediums/ble_frames.proto`):
//!
//! - Weave: every GATT write (phone to us) and every indication (us to the
//!   phone) is one packet with a 1-byte header. Control packets
//!   (`1 CCC NNNN`) open the connection: the phone's ConnectionRequest, our
//!   ConnectionConfirm. Data packets (`0 CCC F L 00`) carry a message in
//!   pieces, F and L mark its first and last packet. Each direction counts
//!   its packets (CCC, mod 8); the request and the confirm are packet 0.
//! - Bluetooth socket: the phone's first message is an INTRODUCTION control
//!   frame (`00 00 00` + `SocketControlFrame`), every other message starts
//!   with the service id hash `FC 9F 5E`. The bytes after the hash, put
//!   together, are the same stream as on TCP: length-prefixed frames.
//!
//! [`Hub`] turns what BlueZ reports ([`gatt::Event`]) into [`WeaveLink`]s,
//! which the handshake and [`Conn`](super::conn::Conn) use like a TCP
//! connection. BlueZ sends an indication to every subscribed phone, so one
//! phone at a time has a link.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::bail;
use prost::Message as _;
use zbus::zvariant::OwnedObjectPath;

use super::conn::Link;
use super::endpoint::SERVICE_HASH;
use super::frame::MAX_FRAME;
use super::{IO_TIMEOUT, gatt};

/// Sends one packet to the phone as an indication.
pub type Indicate = Arc<dyn Fn(&[u8]) -> io::Result<()> + Send + Sync>;
/// Sends one packet the way the side of a link does (indication or write).
type Transmit = Box<dyn Fn(&[u8]) -> io::Result<()> + Send + Sync>;

const CONTROL: u8 = 0x80;
const COMMAND: u8 = 0x0F;
const FIRST: u8 = 0x08;
const LAST: u8 = 0x04;
const REQUEST: u8 = 0;
const CONFIRM: u8 = 1;
const ERROR: u8 = 2;
const VERSION: u16 = 1;
/// The ATT MTU until the phone negotiates a larger one.
const DEFAULT_MTU: u16 = 23;
/// An attribute value, so a packet, is at most 512 bytes.
const MAX_PACKET: usize = 512;
/// What a socket control frame starts with.
const CONTROL_PREFIX: [u8; 3] = [0; 3];
/// The largest message: a frame with its length and the hash in front.
const MAX_MESSAGE: usize = MAX_FRAME + 4 + SERVICE_HASH.len();
/// At most this much read and not yet taken: a phone that sends further
/// ahead loses its link.
const MAX_INBOUND: usize = 2 * MAX_MESSAGE;
/// `SocketControlFrame.ControlFrameType`.
const INTRODUCTION: i32 = 1;
const DISCONNECTION: i32 = 2;
#[cfg(test)]
const SOCKET_VERSION_2: i32 = 2;
/// How long an indication may wait for the phone's confirmation.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

/// `location.nearby.mediums.SocketControlFrame`, the fields read here.
#[derive(Clone, PartialEq, prost::Message)]
struct SocketControlFrame {
    #[prost(int32, optional, tag = "1")]
    r#type: Option<i32>,
    #[prost(message, optional, tag = "2")]
    introduction: Option<IntroductionFrame>,
}

/// `location.nearby.mediums.IntroductionFrame`.
#[derive(Clone, PartialEq, prost::Message)]
struct IntroductionFrame {
    #[prost(bytes = "vec", optional, tag = "1")]
    service_id_hash: Option<Vec<u8>>,
    #[prost(int32, optional, tag = "2")]
    socket_version: Option<i32>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn data_header(counter: u8, first: bool, last: bool) -> u8 {
    ((counter & 7) << 4) | if first { FIRST } else { 0 } | if last { LAST } else { 0 }
}

/// `msg` as data packets of at most `size` bytes, numbered on from `counter`.
fn fragment(msg: &[u8], size: usize, counter: &mut u8) -> Vec<Vec<u8>> {
    let pieces: Vec<&[u8]> = if msg.is_empty() {
        vec![&[]]
    } else {
        msg.chunks(size.saturating_sub(1).max(1)).collect()
    };
    let n = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, piece)| {
            let mut packet = Vec::with_capacity(1 + piece.len());
            packet.push(data_header(*counter, i == 0, i + 1 == n));
            packet.extend_from_slice(piece);
            *counter = (*counter + 1) % 8;
            packet
        })
        .collect()
}

/// A phone's ConnectionRequest.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    min_version: u16,
    max_version: u16,
    /// 0: no limit of its own.
    max_packet: u16,
}

/// `80 minVersion maxVersion maxPacketSize [data]`, all u16 big-endian.
fn parse_request(packet: &[u8]) -> Option<Request> {
    if packet.len() < 7 || packet[0] != CONTROL | REQUEST {
        return None;
    }
    let u16_at = |i: usize| u16::from_be_bytes([packet[i], packet[i + 1]]);
    Some(Request {
        min_version: u16_at(1),
        max_version: u16_at(3),
        max_packet: u16_at(5),
    })
}

/// `81 version packetSize`.
fn confirm(packet_size: usize) -> Vec<u8> {
    let mut packet = vec![CONTROL | CONFIRM];
    packet.extend(VERSION.to_be_bytes());
    packet.extend(u16::try_from(packet_size).unwrap_or(u16::MAX).to_be_bytes());
    packet
}

fn error_packet(counter: u8) -> [u8; 1] {
    [CONTROL | ((counter & 7) << 4) | ERROR]
}

/// The packet size for a connection: what one indication carries at `mtu`
/// (0: not reported), within the phone's own maximum.
fn packet_size(max_packet: u16, mtu: u16) -> usize {
    let mtu = if mtu == 0 { DEFAULT_MTU } else { mtu };
    let size = usize::from(mtu.saturating_sub(3)).min(MAX_PACKET);
    let size = if max_packet == 0 { size } else { size.min(usize::from(max_packet)) };
    size.max(2)
}

/// Puts data packets back together into messages.
struct Reassembler {
    /// The number the next packet must have.
    counter: u8,
    /// The message so far, between its first and its last packet.
    message: Option<Vec<u8>>,
}

impl Reassembler {
    /// After packet 0, the request or the confirm.
    fn new() -> Reassembler {
        Reassembler { counter: 1, message: None }
    }

    /// Takes the next data packet; returns the message its last packet
    /// completes.
    fn push(&mut self, packet: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        let Some((&head, body)) = packet.split_first() else {
            bail!("empty Weave packet");
        };
        if head & CONTROL != 0 {
            bail!("unexpected Weave control packet {head:#04x}");
        }
        let counter = (head >> 4) & 7;
        if counter != self.counter {
            bail!("Weave packet {counter}, expected {}", self.counter);
        }
        self.counter = (self.counter + 1) % 8;
        let mut message = match (head & FIRST != 0, self.message.take()) {
            (true, None) => Vec::new(),
            (false, Some(m)) => m,
            (true, Some(_)) => bail!("a Weave message starts inside another"),
            (false, None) => bail!("a Weave packet outside a message"),
        };
        if message.len() + body.len() > MAX_MESSAGE {
            bail!("a Weave message larger than {MAX_MESSAGE} bytes");
        }
        message.extend_from_slice(body);
        if head & LAST != 0 {
            return Ok(Some(message));
        }
        self.message = Some(message);
        Ok(None)
    }
}

/// A Bluetooth socket message.
#[derive(Debug, PartialEq, Eq)]
enum Message<'a> {
    /// Bytes of the stream, the hash taken off.
    Data(&'a [u8]),
    Introduction {
        hash: Vec<u8>,
    },
    Disconnection,
    /// Another control frame (packet acknowledgement, …).
    Control,
    /// Neither data nor control: for another service.
    Foreign,
}

fn parse_message(msg: &[u8]) -> Message<'_> {
    match msg.split_at_checked(SERVICE_HASH.len()) {
        Some((prefix, rest)) if prefix == SERVICE_HASH => Message::Data(rest),
        Some((prefix, rest)) if prefix == CONTROL_PREFIX => match SocketControlFrame::decode(rest) {
            Ok(f) if f.r#type == Some(INTRODUCTION) => Message::Introduction {
                hash: f.introduction.and_then(|i| i.service_id_hash).unwrap_or_default(),
            },
            Ok(f) if f.r#type == Some(DISCONNECTION) => Message::Disconnection,
            _ => Message::Control,
        },
        _ => Message::Foreign,
    }
}

/// Stream bytes as a message.
fn data_message(bytes: &[u8]) -> Vec<u8> {
    [&SERVICE_HASH[..], bytes].concat()
}

/// The INTRODUCTION a phone sends first.
#[cfg(test)]
fn introduction() -> Vec<u8> {
    let frame = SocketControlFrame {
        r#type: Some(INTRODUCTION),
        introduction: Some(IntroductionFrame {
            service_id_hash: Some(SERVICE_HASH.to_vec()),
            socket_version: Some(SOCKET_VERSION_2),
        }),
    };
    [&CONTROL_PREFIX[..], &frame.encode_to_vec()].concat()
}

/// What one side of a link has read.
enum Got {
    Nothing,
    Data(Vec<u8>),
    Disconnected,
}

/// Reading one side of a link: packets into messages, messages into stream
/// bytes.
struct Receiving {
    packets: Reassembler,
    /// The INTRODUCTION came (or is not expected: we send none).
    introduced: bool,
}

impl Receiving {
    fn new(introduced: bool) -> Receiving {
        Receiving {
            packets: Reassembler::new(),
            introduced,
        }
    }

    fn push(&mut self, packet: &[u8]) -> anyhow::Result<Got> {
        let Some(msg) = self.packets.push(packet)? else {
            return Ok(Got::Nothing);
        };
        Ok(match parse_message(&msg) {
            Message::Data(bytes) => {
                if !self.introduced {
                    log::debug!("quickshare: Bluetooth data before the introduction, taking it");
                    self.introduced = true;
                }
                Got::Data(bytes.to_vec())
            }
            Message::Introduction { hash } if !self.introduced => {
                if hash != SERVICE_HASH {
                    bail!("the Bluetooth connection is for another service ({})", hex(&hash));
                }
                self.introduced = true;
                Got::Nothing
            }
            Message::Disconnection => Got::Disconnected,
            Message::Introduction { .. } | Message::Control => Got::Nothing,
            Message::Foreign => {
                log::debug!("quickshare: skipping a Bluetooth message for another service: {}", hex(&msg));
                Got::Nothing
            }
        })
    }
}

#[derive(Default)]
struct State {
    /// Read, not yet taken.
    inbound: VecDeque<u8>,
    /// Closed here.
    closed: bool,
    /// Ended by the phone (or because of an error).
    ended: bool,
    /// A packet waits for the phone's confirmation.
    in_flight: bool,
}

struct Writer {
    /// The number of the next packet.
    counter: u8,
    /// Written bytes that do not make a whole frame yet.
    pending: Vec<u8>,
}

/// One side of a link, shared by its handles and the [`Hub`].
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    writer: Mutex<Writer>,
    /// Sends one packet.
    transmit: Transmit,
    /// Each packet waits for the confirmation of the one before
    /// (indications).
    confirmed: bool,
    packet_size: usize,
}

impl Shared {
    fn new(transmit: Transmit, confirmed: bool, packet_size: usize) -> Shared {
        Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            // Packet 0 was the request or the confirm.
            writer: Mutex::new(Writer {
                counter: 1,
                pending: Vec::new(),
            }),
            transmit,
            confirmed,
            packet_size,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }

    /// False (and the link ended) when `data` does not fit within
    /// [`MAX_INBOUND`].
    fn push(&self, data: Vec<u8>) -> bool {
        let mut fits = true;
        self.update(|s| {
            if s.inbound.len() + data.len() > MAX_INBOUND {
                s.ended = true;
                fits = false;
            } else {
                s.inbound.extend(data);
            }
        });
        fits
    }

    fn end(&self) {
        self.update(|s| s.ended = true);
    }

    fn close(&self) {
        self.update(|s| s.closed = true);
    }

    fn confirm(&self) {
        self.update(|s| s.in_flight = false);
    }

    fn is_over(&self) -> bool {
        let s = self.lock();
        s.closed || s.ended
    }

    /// Waits up to [`IO_TIMEOUT`] for bytes, like a socket with a read
    /// timeout; 0 once the link is over.
    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut s = self.lock();
        loop {
            if !s.inbound.is_empty() {
                let n = buf.len().min(s.inbound.len());
                for (b, v) in buf.iter_mut().zip(s.inbound.drain(..n)) {
                    *b = v;
                }
                return Ok(n);
            }
            if s.closed || s.ended {
                return Ok(0);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the phone stopped sending over Bluetooth"));
            };
            s = self.changed.wait_timeout(s, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Takes stream bytes; every whole frame goes out as two messages, its
    /// length and then the frame, as Google's endpoint channel writes them.
    fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let mut w = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let w = &mut *w;
        w.pending.extend_from_slice(buf);
        while let Some(len) = w.pending.get(..4) {
            let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
            if len > MAX_FRAME {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
            }
            if w.pending.len() < 4 + len {
                break;
            }
            let frame: Vec<u8> = w.pending.drain(..4 + len).collect();
            self.send_message(&mut w.counter, &frame[..4])?;
            self.send_message(&mut w.counter, &frame[4..])?;
        }
        Ok(buf.len())
    }

    fn send_message(&self, counter: &mut u8, bytes: &[u8]) -> io::Result<()> {
        for packet in fragment(&data_message(bytes), self.packet_size, counter) {
            self.send_packet(&packet)?;
        }
        Ok(())
    }

    /// Sends a packet once the one before is confirmed.
    fn send_packet(&self, packet: &[u8]) -> io::Result<()> {
        if self.confirmed {
            let deadline = Instant::now() + CONFIRM_TIMEOUT;
            let mut s = self.lock();
            while s.in_flight && !(s.closed || s.ended) {
                let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                    drop(s);
                    self.end();
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the phone did not confirm a Bluetooth indication"));
                };
                s = self.changed.wait_timeout(s, left).unwrap_or_else(PoisonError::into_inner).0;
            }
            if s.closed || s.ended {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            s.in_flight = true;
        } else if self.is_over() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        (self.transmit)(packet).inspect_err(|_| self.end())
    }

    /// Sends a packet without waiting (the [`Hub`] must not wait: it
    /// delivers the confirmations).
    fn send_now(&self, packet: &[u8]) -> io::Result<()> {
        self.update(|s| s.in_flight = self.confirmed);
        (self.transmit)(packet)
    }
}

/// A phone's Bluetooth connection as a [`Link`]. Dropping it closes it.
pub struct WeaveLink {
    shared: Arc<Shared>,
}

impl Read for WeaveLink {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.shared.read(buf)
    }
}

impl Write for WeaveLink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.shared.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Link for WeaveLink {
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(WeaveReader(self.shared.clone())))
    }

    fn close(&self) -> io::Result<()> {
        self.shared.close();
        Ok(())
    }
}

impl Drop for WeaveLink {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// A second handle for reading; dropping it leaves the link open.
struct WeaveReader(Arc<Shared>);

impl Read for WeaveReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// The phone that has the link.
struct Active {
    device: OwnedObjectPath,
    receiving: Receiving,
    shared: Arc<Shared>,
    /// Our ConnectionConfirm.
    confirm: Vec<u8>,
    /// The phone has written since its ConnectionRequest, so it had the
    /// confirm.
    heard: bool,
}

impl Active {
    /// Ends the link after an error, telling the phone if it can be done
    /// without waiting: a writer may hold the counter while it waits for a
    /// confirmation.
    fn fail(&self) {
        let counter = self.shared.writer.try_lock().map(|w| w.counter).ok();
        if let Some(counter) = counter {
            let _ = self.shared.send_now(&error_packet(counter));
        }
        self.shared.end();
    }
}

/// Makes links out of what phones do with the Weave characteristics.
pub struct Hub {
    indicate: Indicate,
    active: Option<Active>,
}

impl Hub {
    pub fn new(indicate: Indicate) -> Hub {
        Hub { indicate, active: None }
    }

    /// Handles one event; returns the link a phone has just opened.
    pub fn handle(&mut self, event: gatt::Event) -> Option<WeaveLink> {
        if self.active.as_ref().is_some_and(|a| a.shared.is_over()) {
            self.active = None;
        }
        match event {
            gatt::Event::Write { device, data, mtu } => return self.write(device, &data, mtu),
            gatt::Event::Confirmed => {
                if let Some(a) = &self.active {
                    a.shared.confirm();
                }
            }
            gatt::Event::Unsubscribed => {
                if let Some(a) = self.active.take() {
                    log::info!("quickshare: {} closed the Bluetooth connection", a.device.as_str());
                    a.shared.end();
                }
            }
            // A confirm indicated before the phone subscribed went nowhere.
            gatt::Event::Subscribed => {
                if let Some(a) = self.active.as_ref().filter(|a| !a.heard) {
                    log::debug!("quickshare: {} subscribed after its ConnectionRequest, confirming again", a.device.as_str());
                    if let Err(e) = a.shared.send_now(&a.confirm) {
                        log::warn!("quickshare: cannot answer {} over Bluetooth: {e}", a.device.as_str());
                    }
                }
            }
            gatt::Event::Released => {}
        }
        None
    }

    fn write(&mut self, device: OwnedObjectPath, data: &[u8], mtu: u16) -> Option<WeaveLink> {
        let head = *data.first()?;
        let ours = self.active.as_ref().is_some_and(|a| a.device == device);
        if head & CONTROL != 0 && head & COMMAND == REQUEST {
            if self.active.is_some() && !ours {
                log::info!("quickshare: {} wants a Bluetooth connection, but another phone has it", device.as_str());
                return None;
            }
            if let Some(old) = self.active.take() {
                log::info!("quickshare: {} opened its Bluetooth connection again", device.as_str());
                old.shared.end();
            }
            return self.open(device, data, mtu);
        }
        if !ours {
            log::debug!("quickshare: Bluetooth write from {} without a connection", device.as_str());
            return None;
        }
        let active = self.active.as_mut()?;
        active.heard = true;
        let got = if head & CONTROL != 0 {
            Err(anyhow::anyhow!("Weave control packet {head:#04x}"))
        } else {
            active.receiving.push(data)
        };
        match got {
            Ok(Got::Data(bytes)) => {
                if !active.shared.push(bytes) {
                    log::error!(
                        "quickshare: {} sends over Bluetooth faster than it is read (over {MAX_INBOUND} bytes ahead), ending the connection",
                        device.as_str()
                    );
                    active.fail();
                    self.active = None;
                }
            }
            Ok(Got::Nothing) => {}
            Ok(Got::Disconnected) => {
                log::info!("quickshare: {} ended the Bluetooth connection", device.as_str());
                active.shared.end();
                self.active = None;
            }
            Err(e) => {
                log::warn!("quickshare: Bluetooth connection with {}: {e:#}", device.as_str());
                active.fail();
                self.active = None;
            }
        }
        None
    }

    fn open(&mut self, device: OwnedObjectPath, data: &[u8], mtu: u16) -> Option<WeaveLink> {
        let Some(request) = parse_request(data) else {
            log::info!("quickshare: bad Weave ConnectionRequest from {}: {}", device.as_str(), hex(data));
            return None;
        };
        if !(request.min_version..=request.max_version).contains(&VERSION) {
            log::warn!(
                "quickshare: {} speaks Weave {}–{} only, not {VERSION}",
                device.as_str(),
                request.min_version,
                request.max_version
            );
            let _ = (self.indicate)(&error_packet(0));
            return None;
        }
        if let Some(extra) = data.get(7..).filter(|d| !d.is_empty()) {
            log::debug!("quickshare: {} sent data with its ConnectionRequest: {}", device.as_str(), hex(extra));
        }
        let size = packet_size(request.max_packet, mtu);
        let indicate = self.indicate.clone();
        let shared = Arc::new(Shared::new(Box::new(move |p| indicate(p)), true, size));
        let confirm = confirm(size);
        if let Err(e) = shared.send_now(&confirm) {
            log::warn!("quickshare: cannot answer {} over Bluetooth: {e}", device.as_str());
            return None;
        }
        log::info!(
            "quickshare: {} connected over Bluetooth (MTU {mtu}, packets of {size} bytes, it allows {})",
            device.as_str(),
            request.max_packet
        );
        self.active = Some(Active {
            device,
            receiving: Receiving::new(false),
            shared: shared.clone(),
            confirm,
            heard: false,
        });
        Some(WeaveLink { shared })
    }
}

impl Drop for Hub {
    /// Without the hub (Bluetooth went away), nothing more arrives.
    fn drop(&mut self) {
        if let Some(a) = self.active.take() {
            a.shared.end();
        }
    }
}

/// The phone's end of a link, for tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;

    pub(crate) const DEVICE: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";

    /// An [`Indicate`] that hands the packets to the receiver.
    pub(crate) fn indications() -> (Indicate, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let indicate: Indicate = Arc::new(move |p: &[u8]| tx.lock().unwrap().send(p.to_vec()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe)));
        (indicate, rx)
    }

    /// A Weave central: writes to a hub that reads `events`, reads that
    /// hub's indications and confirms each one.
    pub(crate) struct Central {
        link: WeaveLink,
        events: mpsc::Sender<gatt::Event>,
        closed: AtomicBool,
    }

    impl Central {
        /// Request, confirm and introduction, with ATT MTU `mtu`.
        pub(crate) fn connect(events: mpsc::Sender<gatt::Event>, indications: mpsc::Receiver<Vec<u8>>, mtu: u16) -> Central {
            let device = OwnedObjectPath::try_from(DEVICE).unwrap();
            let writes = events.clone();
            let write = move |p: &[u8]| {
                let data = p.to_vec();
                writes
                    .send(gatt::Event::Write {
                        device: device.clone(),
                        data,
                        mtu,
                    })
                    .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
            };
            write(&[CONTROL | REQUEST, 0, 1, 0, 1, 0, 0]).unwrap();
            let confirm = indications.recv_timeout(Duration::from_secs(10)).expect("a ConnectionConfirm");
            assert_eq!(confirm[..3], [CONTROL | CONFIRM, 0, 1], "{confirm:02x?}");
            let size = usize::from(u16::from_be_bytes([confirm[3], confirm[4]]));
            events.send(gatt::Event::Confirmed).unwrap();
            let shared = Arc::new(Shared::new(Box::new(write), false, size));
            {
                let mut w = shared.writer.lock().unwrap();
                for p in fragment(&introduction(), size, &mut w.counter) {
                    shared.send_packet(&p).unwrap();
                }
            }
            let (reading, confirms) = (shared.clone(), events.clone());
            thread::spawn(move || {
                let mut receiving = Receiving::new(true);
                loop {
                    match indications.recv_timeout(Duration::from_millis(50)) {
                        Ok(p) => {
                            let _ = confirms.send(gatt::Event::Confirmed);
                            match receiving.push(&p) {
                                Ok(Got::Data(b)) => {
                                    if !reading.push(b) {
                                        return;
                                    }
                                }
                                Ok(Got::Nothing) => {}
                                Ok(Got::Disconnected) | Err(_) => return reading.end(),
                            }
                        }
                        Err(RecvTimeoutError::Timeout) if reading.is_over() => return,
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return reading.end(),
                    }
                }
            });
            Central {
                link: WeaveLink { shared },
                events,
                closed: AtomicBool::new(false),
            }
        }
    }

    impl Read for Central {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.link.read(buf)
        }
    }

    impl Write for Central {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.link.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Link for Central {
        fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
            self.link.reader()
        }

        /// Like a phone that disconnects: indications go off.
        fn close(&self) -> io::Result<()> {
            if !self.closed.swap(true, Ordering::SeqCst) {
                let _ = self.events.send(gatt::Event::Unsubscribed);
            }
            self.link.close()
        }
    }

    impl Drop for Central {
        fn drop(&mut self) {
            let _ = self.close();
        }
    }

    /// Our end and the phone's, with a [`Hub`] on its own thread between
    /// them.
    pub(crate) fn pair(mtu: u16) -> (WeaveLink, Central) {
        let (events, rx) = mpsc::channel();
        let (indicate, indications) = indications();
        let (links_tx, links) = mpsc::channel();
        thread::spawn(move || {
            let mut hub = Hub::new(indicate);
            for e in rx {
                if let Some(link) = hub.handle(e) {
                    let _ = links_tx.send(link);
                }
            }
        });
        let phone = Central::connect(events, indications, mtu);
        (links.recv_timeout(Duration::from_secs(10)).unwrap(), phone)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{DEVICE, indications, pair};
    use super::*;
    use crate::quickshare::frame::{read_frame, write_frame};

    fn device(path: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(path).unwrap()
    }

    fn write(path: &str, data: &[u8], mtu: u16) -> gatt::Event {
        gatt::Event::Write {
            device: device(path),
            data: data.to_vec(),
            mtu,
        }
    }

    #[test]
    fn weave_handshake() {
        let (indicate, sent) = indications();
        let mut hub = Hub::new(indicate);
        // Weave 1–1, the phone allows any packet size; MTU 185.
        let link = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 185));
        assert!(link.is_some());
        assert_eq!(sent.try_recv().unwrap(), [0x81, 0, 1, 0, 182]);

        // A smaller limit of the phone's wins; without a reported MTU, 20.
        assert_eq!(
            parse_request(&[0x80, 0, 1, 0, 2, 0, 100]),
            Some(Request {
                min_version: 1,
                max_version: 2,
                max_packet: 100,
            })
        );
        assert_eq!(packet_size(100, 185), 100);
        assert_eq!(packet_size(0, 0), 20);
        assert_eq!(packet_size(0, 517), 512);
        assert_eq!(confirm(100), [0x81, 0, 1, 0, 100]);

        // Only Weave 2 and up: an error packet, no link.
        let mut hub = Hub::new(hub.indicate.clone());
        assert!(hub.handle(write("/dev/b", &[0x80, 0, 2, 0, 3, 0, 0], 185)).is_none());
        assert_eq!(sent.try_recv().unwrap(), [0x82]);
        assert!(parse_request(&[0x80, 0, 1]).is_none(), "too short");
        assert!(parse_request(&[0x90, 0, 1, 0, 1, 0, 0]).is_none(), "not packet 0");
    }

    #[test]
    fn weave_reassembles_first_last() {
        let mut r = Reassembler::new();
        // 1 = first, 2 = middle, 3 = last; then a one-packet message.
        assert_eq!(r.push(&[0x18, b'a', b'b']).unwrap(), None);
        assert_eq!(r.push(&[0x20, b'c']).unwrap(), None);
        assert_eq!(r.push(&[0x34, b'd']).unwrap(), Some(b"abcd".to_vec()));
        assert_eq!(r.push(&[0x4c, b'e']).unwrap(), Some(b"e".to_vec()));
        // The counter wraps after 7.
        for c in 5..=7u8 {
            assert_eq!(r.push(&[(c << 4) | 0x0c]).unwrap(), Some(vec![]));
        }
        assert_eq!(r.push(&[0x0c, b'z']).unwrap(), Some(b"z".to_vec()));

        assert!(Reassembler::new().push(&[0x2c]).is_err(), "wrong counter");
        assert!(Reassembler::new().push(&[0x14]).is_err(), "last without first");
        let mut r = Reassembler::new();
        r.push(&[0x18]).unwrap();
        assert!(r.push(&[0x28]).is_err(), "first inside a message");
        assert!(Reassembler::new().push(&[0x81]).is_err(), "control packet");
        assert!(Reassembler::new().push(&[]).is_err());
    }

    #[test]
    fn weave_fragments_at_the_packet_size() {
        let msg: Vec<u8> = (0..10).collect();
        let mut counter = 6;
        let packets = fragment(&msg, 4, &mut counter);
        assert_eq!(packets, vec![vec![0x68, 0, 1, 2], vec![0x70, 3, 4, 5], vec![0x00, 6, 7, 8], vec![0x14, 9]]);
        assert_eq!(counter, 2);
        assert_eq!(fragment(&[1, 2, 3], 4, &mut counter), vec![vec![0x2c, 1, 2, 3]]);
        assert_eq!(fragment(&[], 4, &mut counter), vec![vec![0x3c]]);

        // Back together.
        let mut r = Reassembler { counter: 6, message: None };
        let got: Vec<_> = packets.iter().filter_map(|p| r.push(p).unwrap()).collect();
        assert_eq!(got, vec![msg]);
    }

    #[test]
    fn ble_socket_strips_service_hash() {
        assert_eq!(introduction(), [0, 0, 0, 0x08, 0x01, 0x12, 0x07, 0x0a, 0x03, 0xfc, 0x9f, 0x5e, 0x10, 0x02]);
        assert_eq!(parse_message(&introduction()), Message::Introduction { hash: SERVICE_HASH.to_vec() });
        assert_eq!(data_message(b"xy"), [0xfc, 0x9f, 0x5e, b'x', b'y']);
        assert_eq!(parse_message(&[0xfc, 0x9f, 0x5e, b'x', b'y']), Message::Data(b"xy"));
        // DISCONNECTION, PACKET_ACKNOWLEDGEMENT, something else.
        assert_eq!(parse_message(&[0, 0, 0, 0x08, 0x02]), Message::Disconnection);
        assert_eq!(parse_message(&[0, 0, 0, 0x08, 0x03]), Message::Control);
        assert_eq!(parse_message(&[1, 2, 3, 4]), Message::Foreign);
        assert_eq!(parse_message(&[0xfc]), Message::Foreign);

        let mut r = Receiving::new(false);
        let mut push = |p: &[u8]| match r.push(p).unwrap() {
            Got::Data(b) => Some(b),
            Got::Nothing => None,
            Got::Disconnected => panic!("disconnected"),
        };
        let intro = introduction();
        assert_eq!(push(&[&[0x1c][..], &intro].concat()), None, "the introduction is no data");
        assert_eq!(push(&[0x2c, 0xfc, 0x9f, 0x5e, 1, 2]), Some(vec![1, 2]));
        assert_eq!(push(&[0x3c, 0, 0, 0, 0x08, 0x03]), None, "control frames are skipped");
        assert_eq!(push(&[0x4c, 9, 9, 9, 1]), None, "another service");

        let mut other = Receiving::new(false);
        assert!(other.push(&[0x1c, 0, 0, 0, 0x08, 0x01, 0x12, 0x05, 0x0a, 0x03, 1, 2, 3]).is_err());
    }

    #[test]
    fn link_carries_frames_both_ways() {
        let (mut ours, mut phone) = pair(23);
        // A frame larger than a packet, written in two pieces.
        let big: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let mut framed = Vec::new();
        write_frame(&mut framed, &big).unwrap();
        phone.write_all(&framed[..3]).unwrap();
        phone.write_all(&framed[3..]).unwrap();
        assert_eq!(read_frame(&mut ours).unwrap(), big);

        write_frame(&mut ours, b"pong").unwrap();
        let mut reader = phone.reader().unwrap();
        assert_eq!(read_frame(&mut reader).unwrap(), b"pong");

        // The phone going away ends the stream.
        phone.close().unwrap();
        assert_eq!(ours.read(&mut [0u8; 8]).unwrap(), 0);
        assert!(ours.write_all(&[0, 0, 0, 1, 7]).is_err());
    }

    #[test]
    fn indications_wait_for_the_confirmation() {
        let (indicate, sent) = indications();
        let mut hub = Hub::new(indicate);
        let mut link = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 23)).unwrap();
        sent.try_recv().unwrap();
        let writer = std::thread::spawn(move || {
            write_frame(&mut link, b"hi").unwrap();
            link
        });
        // The confirm itself is not confirmed yet: nothing goes out.
        std::thread::sleep(Duration::from_millis(100));
        assert!(sent.try_recv().is_err());
        hub.handle(gatt::Event::Confirmed);
        assert_eq!(sent.recv_timeout(Duration::from_secs(2)).unwrap(), [0x1c, 0xfc, 0x9f, 0x5e, 0, 0, 0, 2]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(sent.try_recv().is_err(), "one packet at a time");
        hub.handle(gatt::Event::Confirmed);
        assert_eq!(sent.recv_timeout(Duration::from_secs(2)).unwrap(), [0x2c, 0xfc, 0x9f, 0x5e, b'h', b'i']);
        hub.handle(gatt::Event::Confirmed);
        drop(writer.join().unwrap());
    }

    #[test]
    fn one_phone_at_a_time() {
        let (indicate, sent) = indications();
        let mut hub = Hub::new(indicate);
        let mut first = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 23)).unwrap();
        sent.try_recv().unwrap();
        hub.handle(gatt::Event::Confirmed);
        assert!(hub.handle(write("/dev/other", &[0x80, 0, 1, 0, 1, 0, 0], 23)).is_none());
        assert!(sent.try_recv().is_err(), "the other phone gets no answer");
        hub.handle(write("/dev/other", &[0x1c, 0xfc, 0x9f, 0x5e, 1], 23));

        let mut intro = vec![0x1c];
        intro.extend(introduction());
        hub.handle(write(DEVICE, &intro, 23));
        hub.handle(write(DEVICE, &[0x2c, 0xfc, 0x9f, 0x5e, 7], 23));
        let mut b = [0u8; 4];
        assert_eq!(first.read(&mut b).unwrap(), 1);
        assert_eq!(b[0], 7, "only the first phone's data");

        // A wrong counter ends the link with an error packet.
        hub.handle(write(DEVICE, &[0x5c, 0xfc, 0x9f, 0x5e, 8], 23));
        assert_eq!(sent.try_recv().unwrap(), [0x92], "our next packet is 1");
        assert_eq!(first.read(&mut b).unwrap(), 0);

        // Then the other phone may connect.
        assert!(hub.handle(write("/dev/other", &[0x80, 0, 1, 0, 1, 0, 0], 23)).is_some());
    }

    #[test]
    fn dropping_the_hub_ends_the_link() {
        let (indicate, _sent) = indications();
        let mut hub = Hub::new(indicate);
        let mut link = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 23)).unwrap();
        drop(hub);
        assert_eq!(link.read(&mut [0u8; 4]).unwrap(), 0);
    }

    #[test]
    fn dropping_the_link_lets_the_next_phone_in() {
        let (indicate, _sent) = indications();
        let mut hub = Hub::new(indicate);
        drop(hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 23)).unwrap());
        assert!(hub.handle(write("/dev/other", &[0x80, 0, 1, 0, 1, 0, 0], 23)).is_some());
    }

    #[test]
    fn a_late_subscription_gets_the_confirm_again() {
        let (indicate, sent) = indications();
        let mut hub = Hub::new(indicate);
        let _link = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 185)).unwrap();
        let confirm = sent.try_recv().unwrap();
        // The phone subscribes only now: the first confirm went nowhere.
        hub.handle(gatt::Event::Subscribed);
        assert_eq!(sent.try_recv().unwrap(), confirm);

        // Once the phone has written, it had the confirm.
        hub.handle(gatt::Event::Confirmed);
        let mut intro = vec![0x1c];
        intro.extend(introduction());
        hub.handle(write(DEVICE, &intro, 185));
        hub.handle(gatt::Event::Subscribed);
        assert!(sent.try_recv().is_err());
    }

    #[test]
    fn a_phone_far_ahead_of_the_reader_loses_its_link() {
        let (indicate, sent) = indications();
        let mut hub = Hub::new(indicate);
        let mut link = hub.handle(write(DEVICE, &[0x80, 0, 1, 0, 1, 0, 0], 517)).unwrap();
        sent.try_recv().unwrap();
        let mut counter = 1;
        let mut packets = fragment(&introduction(), MAX_PACKET, &mut counter);
        let piece = vec![7u8; 64 * 1024];
        let pieces = 2 * MAX_MESSAGE / piece.len() + 2;
        for _ in 0..pieces {
            packets.extend(fragment(&data_message(&piece), MAX_PACKET, &mut counter));
        }
        for p in &packets {
            hub.handle(write(DEVICE, p, 517));
        }
        assert_eq!(sent.try_recv().unwrap(), [0x92], "an error packet");
        let mut read = 0;
        let mut buf = vec![0; 1 << 20];
        loop {
            match link.read(&mut buf).unwrap() {
                0 => break,
                n => read += n,
            }
        }
        assert!(read <= 2 * MAX_MESSAGE, "the link ended at the limit: {read}");
        assert!(read < pieces * piece.len());
    }
}
