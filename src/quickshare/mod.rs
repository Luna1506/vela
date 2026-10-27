//! Quick Share (Nearby Share over Wi-Fi LAN): protocol types and framing, and
//! the two roles on top of them, receiving ([`inbound`]) and sending
//! ([`outbound`]).

pub mod ble;
pub mod conn;
pub mod desktop;
pub mod endpoint;
pub mod files;
pub mod frame;
pub mod inbound;
pub mod mdns;
pub mod outbound;
pub mod proto;
pub mod secure;
pub mod serve;
pub mod ukey2;

use std::io::{self, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use prost::Message;

use conn::{Conn, Incoming};
pub use endpoint::DeviceKind;
use frame::{read_frame, write_frame};
use proto::connections::connection_response_frame::ResponseStatus;
use proto::connections::os_info::OsType;
use proto::connections::{ConnectionResponseFrame, OfflineFrame, OsInfo, offline_frame, v1_frame};
use proto::sharing;
use proto::sharing::connection_response_frame::Status;
use proto::sharing::paired_key_result_frame;
use proto::sharing::v1_frame::FrameType;

/// A file the other device offers. `name` is already a plain file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferFile {
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// Text the other device offers, with the title from its TextMetadata; the
/// text itself only arrives after accepting ([`Event::Text`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferText {
    Text(String),
    Url(String),
    Other(String),
}

/// An incoming transfer waiting for the user's [`Decision`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub id: u64,
    pub device: String,
    pub kind: DeviceKind,
    pub pin: String,
    pub files: Vec<OfferFile>,
    pub text: Option<OfferText>,
    /// Bytes of all files together.
    pub total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Save into this folder.
    Accept(PathBuf),
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Rejected,
    Cancelled,
    NotEnoughSpace,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Incoming(Offer),
    /// Sending: the PIN, as soon as the handshake is done.
    Pin {
        id: u64,
        pin: String,
    },
    /// At most about ten per second and transfer, plus a final one.
    Progress {
        id: u64,
        done: u64,
        total: u64,
    },
    /// Received text; `kind` is `"text"` or `"url"`.
    Text {
        id: u64,
        kind: &'static str,
        text: String,
    },
    Finished {
        id: u64,
        outcome: Outcome,
        saved: Vec<PathBuf>,
    },
}

/// Read and write timeout on the socket. UKEY2 has none of its own, and once
/// encrypted the peer sends a keep-alive every 10 s, so this much silence
/// means it is gone.
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the peer may take for each setup step (paired key frames,
/// introduction).
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How often waiting loops look at `cancel` and for the user's decision.
const POLL: Duration = Duration::from_millis(250);
/// How long a closing side waits for the peer to close as well, so nothing
/// still in flight is cut off.
const LINGER: Duration = Duration::from_secs(2);
/// Progress events per transfer at most this often (plus a final one).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// How a protocol step ended: with its result, or with the whole transfer.
#[derive(Debug, PartialEq, Eq)]
enum Step<T> {
    Next(T),
    End(Outcome),
}

/// Timeouts for the whole connection, set before anything is read.
fn prepare(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)
}

/// Writes a plaintext `OfflineFrame` (length prefix and message in one write).
fn write_plain(mut stream: &TcpStream, v1: proto::connections::V1Frame) -> io::Result<()> {
    let frame = OfflineFrame {
        version: Some(offline_frame::Version::V1 as i32),
        v1: Some(v1),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &frame.encode_to_vec())?;
    stream.write_all(&buf)
}

/// Reads a plaintext `OfflineFrame` straight off the socket: nothing is
/// buffered, so the encrypted frames after it are left for [`Conn`].
fn read_plain(mut stream: &TcpStream) -> anyhow::Result<proto::connections::V1Frame> {
    let frame = read_frame(&mut stream).context("reading a plaintext frame")?;
    let frame = OfflineFrame::decode(frame.as_slice()).context("decoding OfflineFrame")?;
    frame.v1.context("OfflineFrame without v1")
}

/// The plaintext ConnectionResponse both sides send after UKEY2.
fn connection_response() -> proto::connections::V1Frame {
    #[allow(deprecated)]
    let response = ConnectionResponseFrame {
        // Deprecated, but NearDrop sets it and Android is happy with it.
        status: Some(0),
        response: Some(ResponseStatus::Accept as i32),
        os_info: Some(OsInfo {
            r#type: Some(OsType::Linux as i32),
        }),
        ..ConnectionResponseFrame::default()
    };
    proto::connections::V1Frame {
        r#type: Some(v1_frame::FrameType::ConnectionResponse as i32),
        connection_response: Some(response),
        ..Default::default()
    }
}

fn check_connection_response(v1: &proto::connections::V1Frame) -> anyhow::Result<()> {
    if v1.r#type() != v1_frame::FrameType::ConnectionResponse {
        bail!("expected a ConnectionResponse, got {}", v1.r#type().as_str_name());
    }
    match v1.connection_response.as_ref().map(|r| r.response()) {
        Some(ResponseStatus::Accept) => Ok(()),
        _ => bail!("the other device refused the connection"),
    }
}

fn sharing_frame(v1: sharing::V1Frame) -> sharing::Frame {
    sharing::Frame {
        version: Some(sharing::frame::Version::V1 as i32),
        v1: Some(v1),
    }
}

/// Random contents of the sizes NearDrop uses; without a Google account
/// there is nothing real to put here, and Android accepts it.
fn paired_key_encryption() -> sharing::Frame {
    sharing_frame(sharing::V1Frame {
        r#type: Some(FrameType::PairedKeyEncryption as i32),
        paired_key_encryption: Some(sharing::PairedKeyEncryptionFrame {
            secret_id_hash: Some(rand::random::<[u8; 6]>().to_vec()),
            signed_data: Some(rand::random::<[u8; 72]>().to_vec()),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn paired_key_result() -> sharing::Frame {
    sharing_frame(sharing::V1Frame {
        r#type: Some(FrameType::PairedKeyResult as i32),
        paired_key_result: Some(sharing::PairedKeyResultFrame {
            status: Some(paired_key_result_frame::Status::Unable as i32),
            os_type: None,
        }),
        ..Default::default()
    })
}

/// The receiver's answer to an introduction.
fn response(status: Status) -> sharing::Frame {
    sharing_frame(sharing::V1Frame {
        r#type: Some(FrameType::Response as i32),
        connection_response: Some(sharing::ConnectionResponseFrame {
            status: Some(status as i32),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn is_cancel(f: &sharing::Frame) -> bool {
    f.v1.as_ref().is_some_and(|v1| v1.r#type() == FrameType::Cancel)
}

/// Tells the peer the transfer is cancelled; the connection is closed later
/// with [`close`]. Errors do not matter here: the transfer is over either way.
fn send_cancel(conn: &mut Conn) {
    let _ = conn.send_sharing(&sharing_frame(sharing::V1Frame {
        r#type: Some(FrameType::Cancel as i32),
        ..Default::default()
    }));
}

/// Sends DISCONNECTION and waits up to [`LINGER`] for the peer to close too,
/// so the last frames are read before the socket goes away.
fn close(conn: &mut Conn) {
    let _ = conn.send_disconnect();
    let deadline = Instant::now() + LINGER;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match conn.recv(left) {
            Ok(Some(Incoming::Disconnected) | None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }
}

/// The next `sharing.nearby` frame during setup, within [`STEP_TIMEOUT`].
/// A CANCEL from the peer, the peer closing the connection or `cancel` end
/// the transfer as cancelled (the peer is told about a local cancel).
fn next_frame(conn: &mut Conn, cancel: &AtomicBool) -> anyhow::Result<Step<sharing::V1Frame>> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        if cancel.load(Ordering::SeqCst) {
            send_cancel(conn);
            return Ok(Step::End(Outcome::Cancelled));
        }
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
            bail!("the other device stopped answering");
        };
        match conn.recv(left.min(POLL))? {
            Some(Incoming::Sharing(f)) if is_cancel(&f) => return Ok(Step::End(Outcome::Cancelled)),
            Some(Incoming::Sharing(sharing::Frame { v1: Some(v1), .. })) => return Ok(Step::Next(v1)),
            Some(Incoming::Disconnected) => return Ok(Step::End(Outcome::Cancelled)),
            _ => {}
        }
    }
}

/// Progress of one transfer, sent at most every [`PROGRESS_EVERY`].
struct Progress {
    id: u64,
    done: u64,
    total: u64,
    last: Option<Instant>,
}

impl Progress {
    fn new(id: u64, total: u64) -> Progress {
        Progress {
            id,
            done: 0,
            total,
            last: None,
        }
    }

    fn add(&mut self, n: u64, events: &mpsc::Sender<Event>) {
        self.done = self.done.saturating_add(n);
        if self.last.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) {
            self.last = Some(Instant::now());
            self.send(events);
        }
    }

    /// The final event, sent whatever the rate.
    fn send(&self, events: &mpsc::Sender<Event>) {
        let _ = events.send(Event::Progress {
            id: self.id,
            done: self.done,
            total: self.total,
        });
    }
}
