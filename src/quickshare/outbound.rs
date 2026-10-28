//! Sending files to another device: we are the client, the other device the
//! server. ConnectionRequest → UKEY2 (PIN) → ConnectionResponse → paired key
//! frames → Introduction → its Response → file chunks → DISCONNECTION.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, bail};

use super::conn::{Conn, Incoming, Link};
use super::endpoint::EndpointInfo;
use super::proto::connections::connection_request_frame::Medium;
use super::proto::connections::{ConnectionRequestFrame, V1Frame, v1_frame};
use super::proto::sharing;
use super::proto::sharing::connection_response_frame::Status;
use super::proto::sharing::file_metadata::Type as FileType;
use super::proto::sharing::v1_frame::FrameType;
use super::{
    Event, Outcome, POLL, Progress, Step, check_connection_response, close, connection_response, is_cancel, next_frame, paired_key_encryption,
    paired_key_result, prepare, read_plain, send_cancel, sharing_frame, ukey2, write_plain,
};

/// File payloads go out in chunks of this size, like Android's.
const CHUNK: usize = 512 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    /// Test seam: sets `cancel` once the first file chunk is out, so a test
    /// can cancel in the middle of a transfer without racing it.
    pub(super) static CANCEL_AFTER_FIRST_CHUNK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A file being sent.
struct Outgoing {
    file: File,
    size: u64,
    meta: sharing::FileMetadata,
}

/// Sends `files` to the device listening at `addr`, presenting ourselves as
/// `me` with `endpoint_id`. Emits `Pin` once the handshake is done, then
/// `Progress`, and always exactly one `Finished` (also on errors, which are
/// returned as well).
pub fn send(
    addr: SocketAddr,
    me: &EndpointInfo,
    endpoint_id: [u8; 4],
    files: &[PathBuf],
    id: u64,
    events: mpsc::Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut conn = None;
    let result = run(addr, me, endpoint_id, files, id, &events, &cancel, &mut conn);
    let outcome = match &result {
        Ok(outcome) => outcome.clone(),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    };
    if let (Some(conn), Err(_)) = (conn.as_mut(), &result) {
        send_cancel(conn);
    }
    let _ = events.send(Event::Finished {
        id,
        outcome,
        saved: Vec::new(),
    });
    if let Some(conn) = conn.as_mut() {
        close(conn);
    }
    result.map(drop)
}

#[allow(clippy::too_many_arguments)]
fn run(
    addr: SocketAddr,
    me: &EndpointInfo,
    endpoint_id: [u8; 4],
    files: &[PathBuf],
    id: u64,
    events: &mpsc::Sender<Event>,
    cancel: &AtomicBool,
    slot: &mut Option<Conn>,
) -> anyhow::Result<Outcome> {
    let mut outgoing = open_files(files)?;
    let on_pin = |pin| {
        let _ = events.send(Event::Pin { id, pin });
    };
    let conn = match connect(addr, me, endpoint_id, cancel, on_pin)? {
        Step::Next(conn) => slot.insert(conn),
        Step::End(outcome) => return Ok(outcome),
    };
    conn.send_sharing(&introduction(&outgoing))?;
    if let Step::End(outcome) = await_response(conn, cancel)? {
        return Ok(outcome);
    }
    send_files(conn, &mut outgoing, id, events, cancel)
}

/// Connects and runs everything up to the Introduction: ConnectionRequest,
/// UKEY2 (`on_pin` gets the PIN), the ConnectionResponses and the paired key
/// frames.
pub(super) fn connect(
    addr: SocketAddr,
    me: &EndpointInfo,
    endpoint_id: [u8; 4],
    cancel: &AtomicBool,
    on_pin: impl FnOnce(String),
) -> anyhow::Result<Step<Conn>> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).with_context(|| format!("connecting to {addr}"))?;
    prepare(&stream)?;
    connect_link(stream, me, endpoint_id, cancel, on_pin)
}

/// [`connect`] over a link that is already open.
pub(super) fn connect_link(
    mut link: impl Link,
    me: &EndpointInfo,
    endpoint_id: [u8; 4],
    cancel: &AtomicBool,
    on_pin: impl FnOnce(String),
) -> anyhow::Result<Step<Conn>> {
    write_plain(&mut link, connection_request(me, endpoint_id))?;
    let mut reader = link.reader()?;
    let keys = ukey2::client(&mut reader, &mut link)?;
    on_pin(ukey2::pin(&keys.auth));
    write_plain(&mut link, connection_response())?;
    check_connection_response(&read_plain(&mut reader)?)?;
    drop(reader);

    let mut conn = Conn::new(Box::new(link), &keys)?;
    conn.send_sharing(&paired_key_encryption())?;
    let (mut got_encryption, mut got_result) = (false, false);
    while !(got_encryption && got_result) {
        let v1 = match next_frame(&mut conn, cancel)? {
            Step::Next(v1) => v1,
            Step::End(outcome) => return Ok(Step::End(outcome)),
        };
        match v1.r#type() {
            FrameType::PairedKeyEncryption if !got_encryption => {
                conn.send_sharing(&paired_key_result())?;
                got_encryption = true;
            }
            FrameType::PairedKeyResult => got_result = true,
            _ => {}
        }
    }
    Ok(Step::Next(conn))
}

/// Waits for the receiver's answer to the Introduction, as long as the user
/// on the other side takes (the connection stays alive meanwhile).
pub(super) fn await_response(conn: &mut Conn, cancel: &AtomicBool) -> anyhow::Result<Step<()>> {
    loop {
        if cancel.load(Ordering::SeqCst) {
            send_cancel(conn);
            return Ok(Step::End(Outcome::Cancelled));
        }
        let frame = match conn.recv(POLL)? {
            Some(Incoming::Sharing(f)) if is_cancel(&f) => return Ok(Step::End(Outcome::Cancelled)),
            Some(Incoming::Sharing(sharing::Frame { v1: Some(v1), .. })) if v1.r#type() == FrameType::Response => v1,
            Some(Incoming::Disconnected) => return Ok(Step::End(Outcome::Cancelled)),
            _ => continue,
        };
        let status = frame.connection_response.map(|r| r.status()).unwrap_or(Status::Unknown);
        return Ok(match status {
            Status::Accept => Step::Next(()),
            Status::Reject | Status::Unknown => Step::End(Outcome::Rejected),
            Status::NotEnoughSpace => Step::End(Outcome::NotEnoughSpace),
            Status::TimedOut => Step::End(Outcome::Failed("the other device did not answer in time".into())),
            Status::UnsupportedAttachmentType => Step::End(Outcome::Failed("the other device cannot receive this kind of file".into())),
        });
    }
}

fn send_files(conn: &mut Conn, outgoing: &mut [Outgoing], id: u64, events: &mpsc::Sender<Event>, cancel: &AtomicBool) -> anyhow::Result<Outcome> {
    let total = outgoing.iter().map(|o| o.size).sum();
    let mut progress = Progress::new(id, total);
    let mut buf = vec![0u8; CHUNK];
    for o in outgoing {
        let payload_id = o.meta.payload_id();
        let size = o.meta.size();
        let mut offset: u64 = 0;
        while offset < o.size {
            if let Some(outcome) = interrupted(conn, cancel)? {
                return Ok(outcome);
            }
            let want = usize::try_from(o.size - offset).map_or(CHUNK, |left| left.min(CHUNK));
            let n = o.file.read(&mut buf[..want])?;
            if n == 0 {
                bail!("{} got shorter while it was being sent", o.meta.name());
            }
            conn.send_file_chunk(payload_id, size, i64::try_from(offset)?, &buf[..n], false)?;
            offset += n as u64;
            progress.add(n as u64, events);
            #[cfg(test)]
            if CANCEL_AFTER_FIRST_CHUNK.get() {
                cancel.store(true, Ordering::SeqCst);
            }
        }
        if let Some(outcome) = interrupted(conn, cancel)? {
            return Ok(outcome);
        }
        conn.send_file_chunk(payload_id, size, i64::try_from(offset)?, &[], true)?;
    }
    progress.send(events);
    Ok(Outcome::Done)
}

/// Between chunks: answers keep-alives and notices a local cancel or the
/// receiver cancelling or going away.
fn interrupted(conn: &mut Conn, cancel: &AtomicBool) -> anyhow::Result<Option<Outcome>> {
    if cancel.load(Ordering::SeqCst) {
        send_cancel(conn);
        return Ok(Some(Outcome::Cancelled));
    }
    Ok(match conn.recv(Duration::ZERO)? {
        Some(Incoming::Sharing(f)) if is_cancel(&f) => Some(Outcome::Cancelled),
        Some(Incoming::PayloadCancelled { .. } | Incoming::Disconnected) => Some(Outcome::Cancelled),
        _ => None,
    })
}

fn open_files(paths: &[PathBuf]) -> anyhow::Result<Vec<Outgoing>> {
    let mut ids = HashSet::new();
    paths
        .iter()
        .map(|path| {
            let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
            let info = file.metadata().with_context(|| format!("reading {}", path.display()))?;
            if !info.is_file() {
                bail!("{} is not a file", path.display());
            }
            let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
            let mime = mime_type(Path::new(&name));
            let payload_id = loop {
                let id = rand::random::<i64>();
                if ids.insert(id) {
                    break id;
                }
            };
            let meta = sharing::FileMetadata {
                r#type: Some(file_type(Path::new(&name), mime) as i32),
                name: Some(name),
                payload_id: Some(payload_id),
                size: Some(i64::try_from(info.len())?),
                mime_type: Some(mime.into()),
                ..Default::default()
            };
            Ok(Outgoing { file, size: info.len(), meta })
        })
        .collect()
}

fn connection_request(me: &EndpointInfo, endpoint_id: [u8; 4]) -> V1Frame {
    V1Frame {
        r#type: Some(v1_frame::FrameType::ConnectionRequest as i32),
        connection_request: Some(ConnectionRequestFrame {
            endpoint_id: Some(String::from_utf8_lossy(&endpoint_id).into_owned()),
            endpoint_name: Some(me.name.clone().unwrap_or_default()),
            endpoint_info: Some(me.encode()),
            mediums: vec![Medium::WifiLan as i32],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn introduction(outgoing: &[Outgoing]) -> sharing::Frame {
    sharing_frame(sharing::V1Frame {
        r#type: Some(FrameType::Introduction as i32),
        introduction: Some(sharing::IntroductionFrame {
            file_metadata: outgoing.iter().map(|o| o.meta.clone()).collect(),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// The MIME type for a file name's extension; the receiver uses it for
/// icons and to pick an app.
fn mime_type(name: &Path) -> &'static str {
    let ext = name.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

fn file_type(name: &Path, mime: &str) -> FileType {
    let apk = name.extension().is_some_and(|e| e.eq_ignore_ascii_case("apk"));
    match mime.split('/').next() {
        Some("image") => FileType::Image,
        Some("video") => FileType::Video,
        Some("audio") => FileType::Audio,
        _ if apk => FileType::AndroidApp,
        _ => FileType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_and_type_follow_the_extension() {
        let cases = [
            ("IMG_1234.JPG", "image/jpeg", FileType::Image),
            ("clip.mp4", "video/mp4", FileType::Video),
            ("song.opus", "audio/opus", FileType::Audio),
            ("app.apk", "application/vnd.android.package-archive", FileType::AndroidApp),
            ("notes.txt", "text/plain", FileType::Unknown),
            ("archive.tar.zst", "application/octet-stream", FileType::Unknown),
            ("Makefile", "application/octet-stream", FileType::Unknown),
        ];
        for (name, mime, kind) in cases {
            let m = mime_type(Path::new(name));
            assert_eq!((m, file_type(Path::new(name), m)), (mime, kind), "{name}");
        }
    }

    #[test]
    fn introduction_describes_each_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, [1, 2, 3]).unwrap();
        std::fs::write(&b, []).unwrap();
        let outgoing = open_files(&[a, b]).unwrap();
        let intro = introduction(&outgoing).v1.unwrap().introduction.unwrap();
        let files = &intro.file_metadata;
        assert_eq!((files[0].name(), files[0].size(), files[0].mime_type()), ("a.png", 3, "image/png"));
        assert_eq!(files[0].r#type(), FileType::Image);
        assert_eq!((files[1].name(), files[1].size()), ("b.bin", 0));
        assert_ne!(files[0].payload_id, files[1].payload_id);
        assert!(intro.text_metadata.is_empty());
    }

    #[test]
    fn a_folder_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_files(&[dir.path().to_path_buf()]).is_err());
    }

    #[test]
    fn connection_request_carries_what_android_needs() {
        let me = EndpointInfo {
            kind: super::super::DeviceKind::Laptop,
            name: Some("luna".into()),
        };
        let req = connection_request(&me, *b"AB12").connection_request.unwrap();
        assert_eq!((req.endpoint_id(), req.endpoint_name()), ("AB12", "luna"));
        assert_eq!(EndpointInfo::decode(req.endpoint_info()), Some(me));
        assert_eq!(req.mediums, vec![Medium::WifiLan as i32]);
    }
}
