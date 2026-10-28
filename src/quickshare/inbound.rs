//! Receiving files and text: the other device is the client, we are the
//! server. ConnectionRequest → UKEY2 → ConnectionResponse → paired key
//! frames → Introduction → the user decides → Response → payloads. Over
//! Bluetooth, an accepted transfer is offered to go on over Wi-Fi LAN
//! ([`bwu`]).

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};

use anyhow::{Context, bail};

use super::bwu;
use super::conn::{Conn, Incoming, Link};
use super::endpoint::{DeviceKind, EndpointInfo};
use super::files::{self, Partial};
use super::proto::connections::connection_request_frame::Medium;
use super::proto::connections::{V1Frame, v1_frame};
use super::proto::sharing;
use super::proto::sharing::connection_response_frame::Status;
use super::proto::sharing::text_metadata::Type as TextType;
use super::proto::sharing::v1_frame::FrameType;
use super::{
    Decision, Event, Offer, OfferFile, OfferText, Outcome, POLL, Progress, Step, check_connection_response, close, connection_response, is_cancel, next_frame,
    paired_key_encryption, paired_key_result, prepare, read_plain, response, send_cancel, ukey2, write_plain,
};

#[cfg(test)]
thread_local! {
    /// Test seam: the free space `handle` sees, instead of the real one.
    static FREE_SPACE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    /// Test seam: the tests' receiver runs `handle_link` instead of `handle`.
    static VIA_LINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs one incoming connection. Announces the offer as `Incoming` and then
/// waits for a [`Decision`] on `decisions`; every announced offer ends with
/// exactly one `Finished`. Nothing is announced for a connection that ends
/// before its introduction or offers something unsupported (Wi-Fi
/// credentials are refused right away). Errors are returned after
/// `Finished(Failed)`.
pub fn handle(stream: TcpStream, id: u64, events: mpsc::Sender<Event>, decisions: mpsc::Receiver<Decision>, cancel: Arc<AtomicBool>) -> anyhow::Result<()> {
    prepare(&stream)?;
    handle_link(stream, id, events, decisions, cancel, None)
}

/// [`handle`] for serve's port, where a phone that moves a Bluetooth
/// transfer to Wi-Fi LAN connects too: its connection (the first frame is a
/// CLIENT_INTRODUCTION) goes to the waiting session through `rendezvous`.
pub fn handle_tcp(
    stream: TcpStream,
    id: u64,
    events: mpsc::Sender<Event>,
    decisions: mpsc::Receiver<Decision>,
    cancel: Arc<AtomicBool>,
    rendezvous: &bwu::Rendezvous,
) -> anyhow::Result<()> {
    prepare(&stream)?;
    let first = read_plain(&stream)?;
    if let Some(ids) = bwu::client_introduction(&first) {
        if !rendezvous.hand_over(&ids, stream)? {
            log::info!("quickshare: {ids:?} wants to go on over Wi-Fi, but no transfer of it is waiting for that");
        }
        return Ok(());
    }
    session(stream, &first, id, events, decisions, cancel, None)
}

/// [`handle`] over any link (Bluetooth: a [`WeaveLink`](super::weave::WeaveLink)).
/// With `upgrade`, an accepted transfer of files is offered to go on over
/// Wi-Fi LAN; it finishes over `link` if the phone does not come.
pub fn handle_link(
    mut link: impl Link,
    id: u64,
    events: mpsc::Sender<Event>,
    decisions: mpsc::Receiver<Decision>,
    cancel: Arc<AtomicBool>,
    upgrade: Option<bwu::Offer>,
) -> anyhow::Result<()> {
    let first = read_plain(&mut link)?;
    session(link, &first, id, events, decisions, cancel, upgrade)
}

/// Everything after the ConnectionRequest `request`.
fn session(
    mut link: impl Link,
    request: &V1Frame,
    id: u64,
    events: mpsc::Sender<Event>,
    decisions: mpsc::Receiver<Decision>,
    cancel: Arc<AtomicBool>,
    upgrade: Option<bwu::Offer>,
) -> anyhow::Result<()> {
    let Peer {
        device,
        kind,
        endpoint,
        wifi_lan,
    } = sender_of(request)?;
    let mut reader = link.reader()?;
    let keys = ukey2::server(&mut reader, &mut link)?;
    check_connection_response(&read_plain(&mut reader)?)?;
    write_plain(&mut link, connection_response())?;
    drop(reader);
    let upgrade = upgrade.filter(|_| {
        if !wifi_lan {
            log::info!("quickshare: {device} cannot go on over Wi-Fi LAN");
        }
        wifi_lan
    });

    let mut conn = Conn::new(Box::new(link), &keys)?;
    conn.send_sharing(&paired_key_encryption())?;
    let intro = match introduction(&mut conn, &cancel)? {
        Step::Next(intro) => intro,
        Step::End(_) => {
            close(&mut conn);
            return Ok(());
        }
    };
    let plan = if intro.wifi_credentials_metadata.is_empty() {
        Plan::new(intro).ok_or(Status::UnsupportedAttachmentType)
    } else {
        Err(Status::Reject)
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(status) => {
            conn.send_sharing(&response(status))?;
            close(&mut conn);
            return Ok(());
        }
    };

    let _ = events.send(Event::Incoming(Offer {
        id,
        device,
        kind,
        pin: ukey2::pin(&keys.auth),
        files: plan.files.iter().map(|f| f.offer.clone()).collect(),
        text: plan.text.as_ref().map(|t| t.offer.clone()),
        total: plan.total,
    }));
    let mut saved = Vec::new();
    let upgrade = upgrade.as_ref().map(|offer| (offer, endpoint.as_str()));
    let result = receive(&mut conn, &plan, id, &events, &decisions, &cancel, &mut saved, upgrade);
    let outcome = match &result {
        Ok(outcome) => outcome.clone(),
        Err(e) => {
            send_cancel(&mut conn);
            Outcome::Failed(format!("{e:#}"))
        }
    };
    let _ = events.send(Event::Finished { id, outcome, saved });
    close(&mut conn);
    result.map(drop)
}

/// Who sends, from the ConnectionRequest.
struct Peer {
    /// From the endpoint info, else the endpoint name.
    device: String,
    kind: DeviceKind,
    endpoint: String,
    /// Wi-Fi LAN is among the mediums it can move to (or it names none).
    wifi_lan: bool,
}

fn sender_of(request: &V1Frame) -> anyhow::Result<Peer> {
    if request.r#type() != v1_frame::FrameType::ConnectionRequest {
        bail!("expected a ConnectionRequest, got {}", request.r#type().as_str_name());
    }
    let request = request.connection_request.as_ref().context("ConnectionRequest without content")?;
    let info = request.endpoint_info.as_deref().and_then(EndpointInfo::decode);
    let kind = info.as_ref().map_or(DeviceKind::Unknown, |i| i.kind);
    let device = info.and_then(|i| i.name).or_else(|| request.endpoint_name.clone()).unwrap_or_default();
    let device = files::display(&device, MAX_DEVICE_CHARS);
    let mediums: Vec<&str> = request.mediums().map(|m| m.as_str_name()).collect();
    log::debug!("quickshare: {device} can move to {mediums:?}");
    Ok(Peer {
        device,
        kind,
        endpoint: request.endpoint_id().to_owned(),
        wifi_lan: request.mediums.is_empty() || request.mediums().any(|m| m == Medium::WifiLan),
    })
}

/// Answers the paired key frames until the Introduction arrives.
fn introduction(conn: &mut Conn, cancel: &AtomicBool) -> anyhow::Result<Step<sharing::IntroductionFrame>> {
    let mut answered = false;
    loop {
        let v1 = match next_frame(conn, cancel)? {
            Step::Next(v1) => v1,
            Step::End(outcome) => return Ok(Step::End(outcome)),
        };
        match v1.r#type() {
            FrameType::PairedKeyEncryption if !answered => {
                conn.send_sharing(&paired_key_result())?;
                answered = true;
            }
            FrameType::Introduction => return Ok(Step::Next(v1.introduction.unwrap_or_default())),
            _ => {}
        }
    }
}

/// Most files one offer may announce; more are not supported.
pub const MAX_FILES: usize = 1000;
/// Longest device name, file name and text title shown, in characters.
pub const MAX_DEVICE_CHARS: usize = 64;
pub const MAX_TITLE_CHARS: usize = 256;

/// What an introduction announces, checked.
struct Plan {
    files: Vec<PlannedFile>,
    text: Option<PlannedText>,
    total: u64,
}

struct PlannedFile {
    payload_id: i64,
    offer: OfferFile,
}

struct PlannedText {
    payload_id: i64,
    kind: &'static str,
    offer: OfferText,
}

impl Plan {
    /// Files plus at most one text; anything else (apps, streams, several
    /// texts, nothing at all, broken metadata) is not supported.
    fn new(intro: sharing::IntroductionFrame) -> Option<Plan> {
        if !intro.app_metadata.is_empty() || !intro.stream_metadata.is_empty() || intro.text_metadata.len() > 1 {
            return None;
        }
        if intro.file_metadata.len() > MAX_FILES {
            log::warn!("quickshare: offer of {} files refused (limit {MAX_FILES})", intro.file_metadata.len());
            return None;
        }
        let mut ids = HashSet::new();
        let mut total: u64 = 0;
        let mut files = Vec::new();
        for f in &intro.file_metadata {
            let payload_id = f.payload_id?;
            let size = u64::try_from(f.size()).ok()?;
            if !ids.insert(payload_id) {
                return None;
            }
            total = total.checked_add(size)?;
            files.push(PlannedFile {
                payload_id,
                offer: OfferFile {
                    name: files::sanitize(f.name()),
                    size,
                    mime: f.mime_type().to_owned(),
                },
            });
        }
        let text = match intro.text_metadata.first() {
            None => None,
            Some(t) => {
                let payload_id = t.payload_id?;
                if !ids.insert(payload_id) {
                    return None;
                }
                let title = files::display(t.text_title(), MAX_TITLE_CHARS);
                let (kind, offer) = match t.r#type() {
                    TextType::Text => ("text", OfferText::Text(title)),
                    TextType::Url => ("url", OfferText::Url(title)),
                    _ => ("text", OfferText::Other(title)),
                };
                Some(PlannedText { payload_id, kind, offer })
            }
        };
        if files.is_empty() && text.is_none() {
            return None;
        }
        Some(Plan { files, text, total })
    }
}

/// From the offer to the end of the transfer. With `upgrade` (the offer and
/// the phone's endpoint id), files go on over Wi-Fi LAN if the phone can.
#[allow(clippy::too_many_arguments)]
fn receive(
    conn: &mut Conn,
    plan: &Plan,
    id: u64,
    events: &mpsc::Sender<Event>,
    decisions: &mpsc::Receiver<Decision>,
    cancel: &AtomicBool,
    saved: &mut Vec<PathBuf>,
    upgrade: Option<(&bwu::Offer, &str)>,
) -> anyhow::Result<Outcome> {
    let dir = match decide(conn, decisions, cancel)? {
        Step::Next(dir) => dir,
        Step::End(outcome) => return Ok(outcome),
    };
    let files = match open_files(&dir, plan) {
        Ok(files) => files,
        Err((status, outcome)) => {
            conn.send_sharing(&response(status))?;
            return Ok(outcome);
        }
    };
    if let Some(text) = &plan.text {
        conn.expect_bytes(text.payload_id);
    }
    conn.send_sharing(&response(Status::Accept))?;
    // Text alone is done before an upgrade could be.
    let mut upgrade = match upgrade {
        Some((offer, endpoint)) if !plan.files.is_empty() => bwu::Upgrade::start(conn, offer, endpoint)?,
        _ => None,
    };
    transfer(
        conn,
        files,
        plan.text.as_ref(),
        Progress::new(id, plan.total),
        events,
        cancel,
        saved,
        &mut upgrade,
    )
}

/// Waits for the user while keeping the connection alive. A dropped
/// decision channel counts as rejecting.
fn decide(conn: &mut Conn, decisions: &mpsc::Receiver<Decision>, cancel: &AtomicBool) -> anyhow::Result<Step<PathBuf>> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            send_cancel(conn);
            return Ok(Step::End(Outcome::Cancelled));
        }
        match decisions.try_recv() {
            Ok(Decision::Accept(dir)) => return Ok(Step::Next(dir)),
            Ok(Decision::Reject) | Err(TryRecvError::Disconnected) => {
                conn.send_sharing(&response(Status::Reject))?;
                return Ok(Step::End(Outcome::Rejected));
            }
            Err(TryRecvError::Empty) => {}
        }
        match conn.recv(POLL)? {
            Some(Incoming::Sharing(f)) if is_cancel(&f) => return Ok(Step::End(Outcome::Cancelled)),
            Some(Incoming::Disconnected) => return Ok(Step::End(Outcome::Cancelled)),
            _ => {}
        }
    }
}

/// A file being received.
struct Receiving {
    partial: Partial,
    size: u64,
    written: u64,
}

impl Receiving {
    /// Chunks must come in order and stay within the announced size.
    fn write(&mut self, offset: i64, body: &[u8]) -> anyhow::Result<()> {
        if u64::try_from(offset).ok() != Some(self.written) {
            bail!("file chunk at offset {offset}, expected {}", self.written);
        }
        let end = self.written + body.len() as u64;
        if end > self.size {
            bail!("file is larger than the announced {} bytes", self.size);
        }
        self.partial.file().write_all(body).context("writing a received file")?;
        self.written = end;
        Ok(())
    }

    fn commit(self) -> anyhow::Result<PathBuf> {
        if self.written != self.size {
            bail!("file ended after {} of the announced {} bytes", self.written, self.size);
        }
        self.partial.commit().context("saving a received file")
    }
}

/// Free space as `handle` sees it.
fn free_space(dir: &Path) -> io::Result<u64> {
    #[cfg(test)]
    if let Some(free) = FREE_SPACE.get() {
        return Ok(free);
    }
    files::free_space(dir)
}

/// Checks the space in `dir` and creates a partial file for every announced
/// file, so a missing or read-only folder shows before anything is accepted.
/// On failure, the Response status for the sender and the outcome here.
fn open_files(dir: &Path, plan: &Plan) -> Result<HashMap<i64, Receiving>, (Status, Outcome)> {
    let mut out = HashMap::new();
    if plan.files.is_empty() {
        return Ok(out);
    }
    let unusable = |e: io::Error| (Status::Reject, Outcome::Failed(format!("cannot save to {}: {e}", dir.display())));
    if free_space(dir).map_err(unusable)? < plan.total {
        return Err((Status::NotEnoughSpace, Outcome::NotEnoughSpace));
    }
    let mut taken = HashSet::new();
    for f in &plan.files {
        let path = files::unique_path(dir, &f.offer.name, &taken);
        taken.insert(path.clone());
        let partial = Partial::create(dir, path).map_err(unusable)?;
        out.insert(
            f.payload_id,
            Receiving {
                partial,
                size: f.offer.size,
                written: 0,
            },
        );
    }
    Ok(out)
}

/// Writes the file chunks and collects the text until everything is in,
/// meanwhile moving to Wi-Fi LAN when `upgrade` gets there. Unfinished
/// partial files are deleted when this returns.
#[allow(clippy::too_many_arguments)]
fn transfer(
    conn: &mut Conn,
    mut files: HashMap<i64, Receiving>,
    text: Option<&PlannedText>,
    mut progress: Progress,
    events: &mpsc::Sender<Event>,
    cancel: &AtomicBool,
    saved: &mut Vec<PathBuf>,
    upgrade: &mut Option<bwu::Upgrade>,
) -> anyhow::Result<Outcome> {
    let mut text = text;
    let id = progress.id;
    while !files.is_empty() || text.is_some() {
        if cancel.load(Ordering::SeqCst) {
            send_cancel(conn);
            return Ok(Outcome::Cancelled);
        }
        if let Some(u) = upgrade {
            u.poll(conn)?;
        }
        match conn.recv(POLL)? {
            Some(Incoming::FileChunk {
                payload_id,
                offset,
                body,
                last,
            }) => {
                let file = files
                    .get_mut(&payload_id)
                    .with_context(|| format!("file payload {payload_id} was not announced"))?;
                file.write(offset, &body)?;
                progress.add(body.len() as u64, events);
                if last && let Some(file) = files.remove(&payload_id) {
                    saved.push(file.commit()?);
                }
            }
            Some(Incoming::Bytes { payload_id, data }) => {
                if let Some(t) = text.filter(|t| t.payload_id == payload_id) {
                    let _ = events.send(Event::Text {
                        id,
                        kind: t.kind,
                        text: String::from_utf8_lossy(&data).into_owned(),
                    });
                    text = None;
                }
            }
            Some(Incoming::Sharing(f)) if is_cancel(&f) => return Ok(Outcome::Cancelled),
            Some(Incoming::PayloadCancelled { .. } | Incoming::Disconnected) => return Ok(Outcome::Cancelled),
            Some(Incoming::Upgrade(f)) => {
                if let Some(u) = upgrade {
                    u.on_frame(&f);
                }
            }
            Some(Incoming::Sharing(_)) | None => {}
        }
    }
    progress.send(events);
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quickshare::conn::Conn;
    use crate::quickshare::endpoint::{DeviceKind, EndpointInfo};
    use crate::quickshare::proto::sharing;
    use crate::quickshare::{Offer, OfferFile, OfferText, Outcome, Step, outbound, sharing_frame};
    use rand::RngExt;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::net::{SocketAddr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);
    const RECEIVE_ID: u64 = 7;
    const SEND_ID: u64 = 9;

    struct Receiver {
        addr: SocketAddr,
        events: mpsc::Receiver<Event>,
        decisions: mpsc::Sender<Decision>,
        cancel: Arc<AtomicBool>,
        thread: JoinHandle<anyhow::Result<()>>,
    }

    /// `handle` (or `handle_link`, see [`VIA_LINK`]) on the first
    /// connection to a fresh listener, with `free` overriding the free space
    /// of the target folder.
    fn receiver(free: Option<u64>) -> Receiver {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, events) = mpsc::channel();
        let (decisions, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let via_link = VIA_LINK.get();
        let thread = thread::spawn(move || {
            FREE_SPACE.set(free);
            let (stream, _) = listener.accept()?;
            if via_link {
                handle_link(stream, RECEIVE_ID, tx, rx, c, None)
            } else {
                handle(stream, RECEIVE_ID, tx, rx, c)
            }
        });
        Receiver {
            addr,
            events,
            decisions,
            cancel,
            thread,
        }
    }

    struct Sender {
        events: mpsc::Receiver<Event>,
        thread: JoinHandle<anyhow::Result<()>>,
    }

    fn me() -> EndpointInfo {
        EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("vela-test".into()),
        }
    }

    /// `outbound::send` in a thread; `cancel_after_first_chunk` cancels it
    /// once the first file chunk is out.
    fn sender(addr: SocketAddr, files: &[PathBuf], cancel_after_first_chunk: bool) -> Sender {
        let files = files.to_vec();
        let (tx, events) = mpsc::channel();
        let thread = thread::spawn(move || {
            outbound::CANCEL_AFTER_FIRST_CHUNK.set(cancel_after_first_chunk);
            outbound::send(addr, &me(), *b"VELA", &files, SEND_ID, tx, Arc::new(AtomicBool::new(false)))
        });
        Sender { events, thread }
    }

    /// A hand-made sender: the real handshake, then `intro` as introduction.
    fn stub(addr: SocketAddr, intro: sharing::IntroductionFrame) -> Conn {
        let Step::Next(mut conn) = outbound::connect(addr, &me(), *b"STUB", &AtomicBool::new(false), |_| {}).unwrap() else {
            panic!("handshake ended early");
        };
        conn.send_sharing(&sharing_frame(sharing::V1Frame {
            r#type: Some(sharing::v1_frame::FrameType::Introduction as i32),
            introduction: Some(intro),
            ..Default::default()
        }))
        .unwrap();
        conn
    }

    fn file_meta(name: &str, payload_id: i64, size: i64) -> sharing::FileMetadata {
        sharing::FileMetadata {
            name: Some(name.into()),
            payload_id: Some(payload_id),
            size: Some(size),
            ..Default::default()
        }
    }

    fn files_intro(files: Vec<sharing::FileMetadata>) -> sharing::IntroductionFrame {
        sharing::IntroductionFrame {
            file_metadata: files,
            ..Default::default()
        }
    }

    fn next_event(events: &mpsc::Receiver<Event>) -> Event {
        events.recv_timeout(WAIT).expect("no event in time")
    }

    fn offer(events: &mpsc::Receiver<Event>) -> Offer {
        loop {
            if let Event::Incoming(offer) = next_event(events) {
                return offer;
            }
        }
    }

    fn pin(events: &mpsc::Receiver<Event>) -> String {
        loop {
            if let Event::Pin { id, pin } = next_event(events) {
                assert_eq!(id, SEND_ID);
                return pin;
            }
        }
    }

    struct Finished {
        outcome: Outcome,
        saved: Vec<PathBuf>,
        /// The last progress event before it, as (done, total).
        progress: Option<(u64, u64)>,
        texts: Vec<(&'static str, String)>,
    }

    fn finished(events: &mpsc::Receiver<Event>) -> Finished {
        let mut progress = None;
        let mut texts = Vec::new();
        loop {
            match next_event(events) {
                Event::Finished { outcome, saved, .. } => {
                    return Finished {
                        outcome,
                        saved,
                        progress,
                        texts,
                    };
                }
                Event::Progress { done, total, .. } => progress = Some((done, total)),
                Event::Text { kind, text, .. } => texts.push((kind, text)),
                _ => {}
            }
        }
    }

    /// Every entry in `dir`, hidden ones included, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    fn sha256(path: &Path) -> Vec<u8> {
        Sha256::digest(fs::read(path).unwrap()).to_vec()
    }

    fn random_bytes(n: usize) -> Vec<u8> {
        let mut b = vec![0u8; n];
        rand::rng().fill(&mut b[..]);
        b
    }

    fn join(thread: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
        thread.join().expect("thread panicked")
    }

    #[test]
    fn loopback_transfer_is_byte_identical() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let small = src.path().join("note.txt");
        let big = src.path().join("clip.mp4");
        fs::write(&small, b"x").unwrap();
        fs::write(&big, random_bytes(3 << 20)).unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[small.clone(), big.clone()], false);
        let offer = offer(&r.events);
        assert_eq!(offer.id, RECEIVE_ID);
        assert_eq!((offer.device.as_str(), offer.kind), ("vela-test", DeviceKind::Laptop));
        let expected = vec![
            OfferFile {
                name: "note.txt".into(),
                size: 1,
                mime: "text/plain".into(),
            },
            OfferFile {
                name: "clip.mp4".into(),
                size: 3 << 20,
                mime: "video/mp4".into(),
            },
        ];
        assert_eq!(offer.files, expected);
        assert_eq!((offer.text, offer.total), (None, 1 + (3 << 20)));
        assert_eq!(offer.pin.len(), 4);
        assert_eq!(pin(&s.events), offer.pin);

        r.decisions.send(Decision::Accept(dest.path().to_path_buf())).unwrap();
        let got = finished(&r.events);
        assert_eq!(got.outcome, Outcome::Done);
        assert_eq!(got.saved, vec![dest.path().join("note.txt"), dest.path().join("clip.mp4")]);
        assert_eq!(got.progress, Some((offer.total, offer.total)));
        let sent = finished(&s.events);
        assert_eq!(sent.outcome, Outcome::Done);
        assert_eq!(sent.progress, Some((offer.total, offer.total)));

        assert_eq!(sha256(&dest.path().join("note.txt")), sha256(&small));
        assert_eq!(sha256(&dest.path().join("clip.mp4")), sha256(&big));
        assert_eq!(entries(dest.path()), ["clip.mp4", "note.txt"]);
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    #[test]
    fn reject_reaches_sender() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[file], false);
        offer(&r.events);
        r.decisions.send(Decision::Reject).unwrap();
        assert_eq!(finished(&r.events).outcome, Outcome::Rejected);
        assert_eq!(finished(&s.events).outcome, Outcome::Rejected);
        assert!(entries(dest.path()).is_empty());
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    #[test]
    fn duplicate_names_get_numbered() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("x")).unwrap();
        fs::create_dir(src.path().join("y")).unwrap();
        let first = src.path().join("x/a.txt");
        let second = src.path().join("y/a.txt");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[first, second], false);
        offer(&r.events);
        r.decisions.send(Decision::Accept(dest.path().to_path_buf())).unwrap();
        let got = finished(&r.events);
        assert_eq!(got.outcome, Outcome::Done);
        assert_eq!(got.saved, vec![dest.path().join("a.txt"), dest.path().join("a (1).txt")]);
        assert_eq!(fs::read(dest.path().join("a.txt")).unwrap(), b"first");
        assert_eq!(fs::read(dest.path().join("a (1).txt")).unwrap(), b"second");
        assert_eq!(finished(&s.events).outcome, Outcome::Done);
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    #[test]
    fn hostile_names_stay_inside() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("in");
        fs::create_dir(&dest).unwrap();

        let r = receiver(None);
        let mut conn = stub(r.addr, files_intro(vec![file_meta("../evil", 1, 3)]));
        assert_eq!(offer(&r.events).files[0].name, "evil");
        r.decisions.send(Decision::Accept(dest.clone())).unwrap();
        assert_eq!(outbound::await_response(&mut conn, &AtomicBool::new(false)).unwrap(), Step::Next(()));
        conn.send_file_chunk(1, 3, 0, b"abc", false).unwrap();
        conn.send_file_chunk(1, 3, 3, &[], true).unwrap();
        let got = finished(&r.events);
        assert_eq!(got.outcome, Outcome::Done);
        assert_eq!(got.saved, vec![dest.join("evil")]);
        assert_eq!(fs::read(dest.join("evil")).unwrap(), b"abc");
        assert_eq!(entries(root.path()), ["in"]);
        drop(conn);
        join(r.thread).unwrap();
    }

    #[test]
    fn disconnect_mid_transfer_leaves_no_files() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let big = src.path().join("big.bin");
        fs::write(&big, random_bytes(3 << 20)).unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[big], true);
        offer(&r.events);
        r.decisions.send(Decision::Accept(dest.path().to_path_buf())).unwrap();
        let got = finished(&r.events);
        assert!(matches!(got.outcome, Outcome::Cancelled | Outcome::Failed(_)), "{:?}", got.outcome);
        assert!(got.saved.is_empty());
        assert_eq!(finished(&s.events).outcome, Outcome::Cancelled);
        assert!(entries(dest.path()).is_empty(), "{:?}", entries(dest.path()));
        join(s.thread).unwrap();
        let _ = join(r.thread);
    }

    #[test]
    fn not_enough_space_is_reported() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let r = receiver(Some(0));
        let s = sender(r.addr, &[file], false);
        offer(&r.events);
        r.decisions.send(Decision::Accept(dest.path().to_path_buf())).unwrap();
        assert_eq!(finished(&r.events).outcome, Outcome::NotEnoughSpace);
        assert_eq!(finished(&s.events).outcome, Outcome::NotEnoughSpace);
        assert!(entries(dest.path()).is_empty());
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    #[test]
    fn accept_into_missing_dir_fails_cleanly() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[file], false);
        offer(&r.events);
        r.decisions.send(Decision::Accept(dest.path().join("weg"))).unwrap();
        assert!(matches!(finished(&r.events).outcome, Outcome::Failed(_)));
        let sent = finished(&s.events).outcome;
        assert!(matches!(sent, Outcome::Rejected | Outcome::Failed(_)), "{sent:?}");
        assert!(entries(dest.path()).is_empty());
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    fn numbered_file(i: i64, name: &str) -> sharing::FileMetadata {
        sharing::FileMetadata {
            name: Some(name.into()),
            size: Some(1),
            payload_id: Some(i),
            ..Default::default()
        }
    }

    #[test]
    fn an_offer_of_too_many_files_is_not_supported() {
        let many = |n: usize| files_intro((0..n as i64).map(|i| numbered_file(i + 1, "a.txt")).collect());
        assert_eq!(Plan::new(many(MAX_FILES)).map(|p| p.files.len()), Some(MAX_FILES));
        assert!(Plan::new(many(MAX_FILES + 1)).is_none());
    }

    #[test]
    fn long_titles_and_spoofed_names_are_cut_down() {
        let intro = sharing::IntroductionFrame {
            file_metadata: vec![numbered_file(1, "photo\u{202E}gpj.exe")],
            text_metadata: vec![sharing::TextMetadata {
                text_title: Some("ü".repeat(10_000)),
                r#type: Some(sharing::text_metadata::Type::Text as i32),
                payload_id: Some(2),
                size: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        };
        let plan = Plan::new(intro).unwrap();
        assert_eq!(plan.files[0].offer.name, "photogpj.exe");
        match plan.text.unwrap().offer {
            OfferText::Text(t) => assert_eq!(t.chars().count(), MAX_TITLE_CHARS + 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn wifi_credentials_are_rejected() {
        let r = receiver(None);
        let intro = sharing::IntroductionFrame {
            wifi_credentials_metadata: vec![sharing::WifiCredentialsMetadata {
                ssid: Some("home".into()),
                payload_id: Some(5),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut conn = stub(r.addr, intro);
        let answer = outbound::await_response(&mut conn, &AtomicBool::new(false)).unwrap();
        assert_eq!(answer, Step::End(Outcome::Rejected));
        drop(conn);
        join(r.thread).unwrap();
        let events: Vec<Event> = r.events.try_iter().collect();
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn text_arrives_as_an_event() {
        let r = receiver(None);
        let url = "https://example.org/page";
        let intro = sharing::IntroductionFrame {
            text_metadata: vec![sharing::TextMetadata {
                text_title: Some("example.org".into()),
                r#type: Some(sharing::text_metadata::Type::Url as i32),
                payload_id: Some(11),
                size: Some(url.len() as i64),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut conn = stub(r.addr, intro);
        let offer = offer(&r.events);
        assert_eq!(offer.text, Some(OfferText::Url("example.org".into())));
        assert!(offer.files.is_empty());
        let unused = tempfile::tempdir().unwrap();
        r.decisions.send(Decision::Accept(unused.path().to_path_buf())).unwrap();
        assert_eq!(outbound::await_response(&mut conn, &AtomicBool::new(false)).unwrap(), Step::Next(()));
        conn.send_bytes(11, url.as_bytes()).unwrap();
        let got = finished(&r.events);
        assert_eq!(got.outcome, Outcome::Done);
        assert_eq!(got.texts, vec![("url", url.to_string())]);
        assert!(entries(unused.path()).is_empty());
        drop(conn);
        join(r.thread).unwrap();
    }

    #[test]
    fn cancel_while_deciding_reaches_sender() {
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let r = receiver(None);
        let s = sender(r.addr, &[file], false);
        offer(&r.events);
        r.cancel.store(true, Ordering::SeqCst);
        assert_eq!(finished(&r.events).outcome, Outcome::Cancelled);
        assert_eq!(finished(&s.events).outcome, Outcome::Cancelled);
        join(s.thread).unwrap();
        join(r.thread).unwrap();
    }

    #[test]
    fn more_bytes_than_announced_fail_and_leave_nothing() {
        let dest = tempfile::tempdir().unwrap();
        let r = receiver(None);
        let mut conn = stub(r.addr, files_intro(vec![file_meta("a.txt", 1, 2)]));
        offer(&r.events);
        r.decisions.send(Decision::Accept(dest.path().to_path_buf())).unwrap();
        assert_eq!(outbound::await_response(&mut conn, &AtomicBool::new(false)).unwrap(), Step::Next(()));
        conn.send_file_chunk(1, 2, 0, b"abc", true).unwrap();
        assert!(matches!(finished(&r.events).outcome, Outcome::Failed(_)));
        assert!(entries(dest.path()).is_empty());
        drop(conn);
        assert!(join(r.thread).is_err());
    }

    #[test]
    fn the_same_through_handle_link() {
        VIA_LINK.set(true);
        loopback_transfer_is_byte_identical();
        reject_reaches_sender();
        hostile_names_stay_inside();
        disconnect_mid_transfer_leaves_no_files();
        not_enough_space_is_reported();
        wifi_credentials_are_rejected();
        text_arrives_as_an_event();
        cancel_while_deciding_reaches_sender();
        more_bytes_than_announced_fail_and_leave_nothing();
    }
}
