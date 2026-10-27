//! `vela quickshare serve`: the backend of Quick Share in the shell. Writes
//! one JSON document per line to stdout (see the `*_line`/`*_reply`
//! functions) and reads commands as JSON lines from stdin ([`Cmd`]); logs go
//! to stderr. Listens on `quickshare.port` and announces itself over mDNS;
//! every incoming connection runs on its own thread, so several offers can
//! wait at once. Phones that search over Bluetooth only find us through our
//! GATT service and its advertisements. While searching, a BLE beacon makes
//! phones show up. Ends when stdin closes or on SIGTERM/SIGINT: the
//! announcements are withdrawn, the search and the beacon stopped and running
//! transfers are cancelled (which removes their partial files).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::bail;
use mdns_sd::ServiceDaemon;
use serde::Deserialize;
use serde_json::{Value, json};

use super::desktop::{self, Notice};
use super::endpoint::{DeviceKind, EndpointInfo, SERVICE_TYPE, new_endpoint_id};
use super::mdns::{self, Advertisement, Peer, PeerEvent};
use super::{Decision, Event, Offer, OfferText, Outcome, ble, files, gatt, inbound, outbound};
use crate::config;

/// How long ending waits for transfers to wind down.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(4);
/// How long ending waits for the mDNS daemon.
const MDNS_WAIT: Duration = Duration::from_secs(1);

/// A command from the shell. `req`, where there is one, comes back in the
/// answer.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Cmd {
    /// Look for devices to send to (while "Send" is open): `devices` lines.
    Discover {
        on: bool,
    },
    /// Sends to a device from `devices`: a new transfer, `direction: "out"`.
    Send {
        device: String,
        files: Vec<PathBuf>,
    },
    /// Saves an offer into `dir` (empty = the download folder).
    Accept {
        id: u64,
        #[serde(default)]
        dir: String,
    },
    Reject {
        id: u64,
    },
    /// Cancels a transfer either way, also one still waiting.
    Cancel {
        id: u64,
    },
    /// The subfolders of `dir` (empty = the download folder, created if missing).
    Dirs {
        req: Option<u64>,
        #[serde(default)]
        dir: String,
    },
    Mkdir {
        req: Option<u64>,
        parent: PathBuf,
        name: String,
    },
    /// Lets the user choose files (answered with `picked`).
    Pick {
        req: Option<u64>,
    },
    /// Name and size of files (answered like `pick`).
    Stat {
        req: Option<u64>,
        files: Vec<PathBuf>,
    },
    /// Reads the config again; a new name or port is announced (and a new
    /// `hello` sent).
    Reload,
}

pub fn parse_cmd(line: &str) -> anyhow::Result<Cmd> {
    Ok(serde_json::from_str(line)?)
}

enum Msg {
    Cmd(Cmd),
    /// The error line for a command that could not be read.
    Bad(Value),
    /// stdin closed.
    Eof,
    /// SIGTERM or SIGINT.
    Stop,
    Connection(TcpStream),
    Event(Event),
    /// A transfer's thread is done; every event it sent came before.
    Ended(u64),
    /// From the search with this number.
    Peer(u64, PeerEvent),
    /// A line a worker thread has ready (the answer to `pick`).
    Reply(Value),
    /// The file dialog's worker is done; another dialog may open.
    PickDone,
}

/// Reads commands until `input` ends; a line that is not a command is
/// answered with an error line.
fn read_commands(input: impl BufRead, tx: &mpsc::Sender<Msg>) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg = match parse_cmd(line) {
            Ok(cmd) => Msg::Cmd(cmd),
            Err(e) => {
                log::warn!("quickshare: bad command {line:?}: {e}");
                let req = serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("req").and_then(Value::as_u64));
                Msg::Bad(error_line(format!("bad command: {e}"), json!({ "req": req })))
            }
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::Eof);
}

/// `{"type":"error","message":…}` plus the fields of `extra` (`req`, `id`).
fn error_line(message: String, extra: Value) -> Value {
    let mut v = json!({ "type": "error", "message": message });
    if let (Some(v), Value::Object(extra)) = (v.as_object_mut(), extra) {
        v.extend(extra);
    }
    v
}

fn emit(out: &mut impl Write, v: &Value) -> bool {
    serde_json::to_writer(&mut *out, v).is_ok() && out.write_all(b"\n").is_ok() && out.flush().is_ok()
}

fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The transfer state for how it ended, and the error text.
fn state_of(outcome: &Outcome) -> (&'static str, String) {
    match outcome {
        Outcome::Done => ("done", String::new()),
        Outcome::Rejected => ("rejected", String::new()),
        Outcome::Cancelled => ("cancelled", String::new()),
        Outcome::NotEnoughSpace => ("no-space", String::new()),
        Outcome::Failed(e) => ("failed", e.clone()),
    }
}

fn incoming_line(offer: &Offer) -> Value {
    let text = offer.text.as_ref().map(|t| match t {
        OfferText::Url(title) => json!({ "kind": "url", "title": title }),
        OfferText::Text(title) | OfferText::Other(title) => json!({ "kind": "text", "title": title }),
    });
    let files: Vec<Value> = offer.files.iter().map(|f| json!({ "name": f.name, "size": f.size, "mime": f.mime })).collect();
    json!({
        "type": "incoming",
        "id": offer.id,
        "device": offer.device,
        "kind": offer.kind.id(),
        "pin": offer.pin,
        "files": files,
        "text": text,
        "total": offer.total,
    })
}

fn dirs_reply(req: Option<u64>, dir: &str, default: &Path) -> Value {
    let dir = if dir.is_empty() { default.to_path_buf() } else { PathBuf::from(dir) };
    let listed = if dir == default {
        fs::create_dir_all(&dir).and_then(|()| files::list_dirs(&dir))
    } else {
        files::list_dirs(&dir)
    };
    let (dirs, error) = match listed {
        Ok(dirs) => (dirs, String::new()),
        Err(e) => (Vec::new(), format!("{}: {e}", dir.display())),
    };
    json!({
        "type": "dirs",
        "req": req,
        "dir": lossy(&dir),
        "parent": dir.parent().map(lossy).unwrap_or_default(),
        "dirs": dirs,
        "error": error,
    })
}

fn mkdir_reply(req: Option<u64>, parent: &Path, name: &str) -> Value {
    let (path, error) = match files::make_dir(parent, name) {
        Ok(path) => (lossy(&path), String::new()),
        Err(e) => (String::new(), e.to_string()),
    };
    json!({ "type": "mkdir", "req": req, "path": path, "error": error })
}

/// `files` that are files, with name and size; the others are named in `error`.
fn stat_reply(req: Option<u64>, paths: &[PathBuf]) -> Value {
    let (mut ok, mut problems) = (Vec::new(), Vec::new());
    for p in paths {
        match fs::metadata(p) {
            Ok(m) if m.is_file() => ok.push(json!({
                "path": lossy(p),
                "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "size": m.len(),
            })),
            Ok(_) => problems.push(format!("{} is a folder", p.display())),
            Err(e) => problems.push(format!("{}: {e}", p.display())),
        }
    }
    json!({ "type": "picked", "req": req, "files": ok, "error": problems.join("; ") })
}

/// The answer to `pick`: the chosen files like `stat` (none when the dialog
/// was cancelled), or the reason there was no dialog.
fn pick_reply(req: Option<u64>, picked: anyhow::Result<Vec<PathBuf>>) -> Value {
    match picked {
        Ok(paths) => stat_reply(req, &paths),
        Err(e) => json!({ "type": "picked", "req": req, "files": [], "error": e.to_string() }),
    }
}

/// The notification for files received into `dir`.
fn received_notice(lang: desktop::Lang, n: usize, device: &str, dir: &str) -> Notice {
    Notice {
        summary: desktop::received_summary(lang, n),
        body: format!("{device} · {dir}"),
        action: Some((desktop::open_folder_label(lang).into(), desktop::file_uri(dir))),
    }
}

/// The notification for received text: a link can be opened (web links
/// only), plain text was copied by the shell.
fn text_notice(lang: desktop::Lang, kind: &str, text: &str, device: &str) -> Notice {
    let is_web_link = text.starts_with("https://") || text.starts_with("http://");
    if kind == "url" {
        let action = is_web_link.then(|| (desktop::open_label(lang).into(), text.to_owned()));
        Notice {
            summary: text.to_owned(),
            body: device.to_owned(),
            action,
        }
    } else {
        Notice {
            summary: desktop::text_copied_summary(lang, device),
            body: desktop::preview(text, 80),
            action: None,
        }
    }
}

/// The devices found: named ones by name, then those without a name (`null`)
/// by endpoint id.
fn devices_line(peers: &BTreeMap<String, Peer>) -> Value {
    let mut list: Vec<&Peer> = peers.values().collect();
    list.sort_by_cached_key(|p| (p.name.is_none(), p.name.as_deref().map(str::to_lowercase), p.endpoint.clone()));
    let devices: Vec<Value> = list.iter().map(|p| json!({ "id": p.endpoint, "name": p.name, "kind": p.kind.id() })).collect();
    json!({ "type": "devices", "devices": devices })
}

/// The TCP port phones connect to; accepted connections go to the channel.
struct Listener {
    port: u16,
    socket: TcpListener,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    fn bind(port: u16, tx: mpsc::Sender<Msg>) -> std::io::Result<Listener> {
        let socket = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let port = socket.local_addr()?.port();
        let accepting = socket.try_clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new().name("quickshare-listen".into()).spawn(move || {
            for conn in accepting.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                match conn {
                    Ok(stream) => {
                        if tx.send(Msg::Connection(stream)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("quickshare: accepting a connection: {e}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })?;
        Ok(Listener {
            port,
            socket,
            stop,
            thread: Some(thread),
        })
    }

    /// Stops accepting and closes the port.
    fn stop(&mut self) {
        let Some(thread) = self.thread.take() else { return };
        self.stop.store(true, Ordering::SeqCst);
        // SAFETY: shutting down our own socket; on Linux this wakes the
        // blocked accept() with an error.
        unsafe {
            libc::shutdown(self.socket.as_raw_fd(), libc::SHUT_RDWR);
        }
        let _ = thread.join();
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Transfer {
    direction: &'static str,
    device: String,
    state: &'static str,
    done: u64,
    total: u64,
    pin: String,
    error: String,
    dir: String,
    saved: Vec<String>,
    /// Incoming: the offer arrived; nothing is shown before.
    announced: bool,
    cancel: Arc<AtomicBool>,
    /// Incoming, until the user decided.
    decisions: Option<mpsc::Sender<Decision>>,
    thread: Option<JoinHandle<()>>,
}

impl Transfer {
    fn new(direction: &'static str, cancel: Arc<AtomicBool>, thread: JoinHandle<()>) -> Transfer {
        Transfer {
            direction,
            device: String::new(),
            state: "waiting",
            done: 0,
            total: 0,
            pin: String::new(),
            error: String::new(),
            dir: String::new(),
            saved: Vec::new(),
            announced: false,
            cancel,
            decisions: None,
            thread: Some(thread),
        }
    }

    fn line(&self, id: u64) -> Value {
        json!({
            "type": "transfer",
            "id": id,
            "direction": self.direction,
            "device": self.device,
            "state": self.state,
            "done": self.done,
            "total": self.total,
            "pin": self.pin,
            "error": self.error,
            "dir": self.dir,
            "saved": self.saved,
        })
    }
}

struct Options {
    /// Announce over mDNS and allow searching (off in tests).
    mdns: bool,
    /// The `[quickshare]` settings, read at the start and on `reload`.
    load: Box<dyn Fn() -> config::Quickshare + Send>,
    /// Whether the BLE beacon can run, asked at the start and on `reload`.
    probe_ble: Box<dyn Fn() -> bool + Send>,
    receive_ble: ReceiveBle,
    /// Shows a notification (must not block).
    notify: Box<dyn Fn(Notice) + Send>,
}

/// Makes endpoint `id` with this info visible and connectable over
/// Bluetooth (sending what phones do to the channel) until the returned
/// guard is dropped: [`gatt::Receiver::start`].
type ReceiveBle = Box<dyn Fn([u8; 4], &EndpointInfo, mpsc::Sender<gatt::Event>) -> anyhow::Result<Box<dyn Send>> + Send>;

struct Server<'a, W: Write> {
    out: &'a mut W,
    /// stdout still takes lines.
    connected: bool,
    tx: mpsc::Sender<Msg>,
    load: Box<dyn Fn() -> config::Quickshare + Send>,
    probe_ble: Box<dyn Fn() -> bool + Send>,
    notify: Box<dyn Fn(Notice) + Send>,
    /// A file dialog is open (one at a time).
    picking: bool,
    /// The BLE beacon can run, as `hello` says.
    ble: bool,
    /// On the air while searching.
    beacon: Option<ble::Beacon>,
    receive_ble: ReceiveBle,
    /// Visible over Bluetooth, as `hello` says (`bleReceive`).
    ble_receiver: Option<Box<dyn Send>>,
    /// Where the BLE receiver sends what phones do.
    ble_events: mpsc::Sender<gatt::Event>,
    /// Our endpoint id, for this run.
    id: [u8; 4],
    name: String,
    /// The port from the config (0 in tests: any).
    configured_port: u16,
    listener: Listener,
    daemon: Option<ServiceDaemon>,
    advertisement: Option<Advertisement>,
    /// The number of the running search.
    search: Option<u64>,
    searches: u64,
    peers: BTreeMap<String, Peer>,
    transfers: HashMap<u64, Transfer>,
    last_id: u64,
}

/// Runs until stdin closes, a signal arrives or stdout breaks.
pub fn run() -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();
    if let Err(e) = forward_signals(tx.clone()) {
        log::warn!("quickshare: cannot handle signals: {e}");
    }
    let reader = tx.clone();
    std::thread::spawn(move || read_commands(std::io::stdin().lock(), &reader));
    let opts = Options {
        mdns: true,
        load: Box::new(|| load_config().quickshare),
        probe_ble: Box::new(probe_ble),
        receive_ble: Box::new(|id, info, events| gatt::Receiver::start(id, info, events).map(|r| Box::new(r) as Box<dyn Send>)),
        notify: Box::new(desktop::notify_detached),
    };
    serve(tx, rx, &mut std::io::stdout().lock(), opts)
}

/// Whether BlueZ is there with a powered adapter (nothing is registered).
fn probe_ble() -> bool {
    match ble::probe() {
        Ok(()) => true,
        Err(e) => {
            log::info!("quickshare: no BLE beacon: {e:#}");
            false
        }
    }
}

fn load_config() -> config::Config {
    let file = crate::paths::config_file();
    match fs::read_to_string(&file).map(|t| config::Config::from_toml(&t)) {
        Ok(Ok(cfg)) => cfg,
        Ok(Err(e)) => {
            log::warn!("quickshare: {}: {e:#}; using the defaults", file.display());
            config::Config::default()
        }
        Err(_) => config::Config::default(),
    }
}

fn serve(tx: mpsc::Sender<Msg>, rx: mpsc::Receiver<Msg>, out: &mut impl Write, opts: Options) -> anyhow::Result<()> {
    let mut server = Server::start(tx, out, opts)?;
    while let Ok(msg) = rx.recv() {
        if !server.handle(msg) {
            break;
        }
    }
    server.shutdown(&rx);
    Ok(())
}

impl<'a, W: Write> Server<'a, W> {
    /// Opens the port (or fails: no other port, the firewall is opened for
    /// this one), says hello and announces us.
    fn start(tx: mpsc::Sender<Msg>, out: &'a mut W, opts: Options) -> anyhow::Result<Self> {
        let settings = (opts.load)();
        let ble = (opts.probe_ble)();
        let listener = match Listener::bind(settings.port, tx.clone()) {
            Ok(l) => l,
            Err(e) => {
                let message = format!("cannot listen on port {}: {e}", settings.port);
                emit(out, &error_line(message.clone(), json!({})));
                bail!(message);
            }
        };
        // What phones write over Bluetooth; reading it into connections
        // comes later, until then it is only logged (by gatt).
        let (ble_events, written) = mpsc::channel::<gatt::Event>();
        let drained = std::thread::Builder::new()
            .name("quickshare-ble".into())
            .spawn(move || written.into_iter().for_each(drop));
        if let Err(e) = drained {
            log::warn!("quickshare: cannot read from Bluetooth: {e}");
        }
        let mut server = Server {
            out,
            connected: true,
            tx,
            load: opts.load,
            probe_ble: opts.probe_ble,
            notify: opts.notify,
            picking: false,
            ble,
            beacon: None,
            receive_ble: opts.receive_ble,
            ble_receiver: None,
            ble_events,
            id: new_endpoint_id(),
            name: settings.effective_name(),
            configured_port: settings.port,
            listener,
            daemon: None,
            advertisement: None,
            search: None,
            searches: 0,
            peers: BTreeMap::new(),
            transfers: HashMap::new(),
            last_id: 0,
        };
        log::info!(
            "quickshare: listening on port {} as {:?} (endpoint {})",
            server.listener.port,
            server.name,
            String::from_utf8_lossy(&server.id)
        );
        server.receive_over_ble();
        server.hello();
        if opts.mdns {
            match ServiceDaemon::new() {
                Ok(daemon) => server.daemon = Some(daemon),
                Err(e) => server.error(format!("mDNS is not available: {e}"), json!({})),
            }
            server.advertise();
        }
        Ok(server)
    }

    fn emit(&mut self, v: &Value) -> bool {
        if self.connected {
            self.connected = emit(self.out, v);
        }
        self.connected
    }

    fn error(&mut self, message: String, extra: Value) {
        log::warn!("quickshare: {message}");
        self.emit(&error_line(message, extra));
    }

    fn hello(&mut self) {
        // "ble": whether the beacon that makes phones list us can run;
        // "bleReceive": whether phones searching over Bluetooth find us.
        let hello = json!({
            "type": "hello",
            "name": self.name,
            "port": self.listener.port,
            "ble": self.ble,
            "bleReceive": self.ble_receiver.is_some(),
        });
        self.emit(&hello);
    }

    fn me(&self) -> EndpointInfo {
        EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some(self.name.clone()),
        }
    }

    /// (Re)starts the BLE receiver under the current name. Without it
    /// phones that search over Bluetooth only do not find us; the shell
    /// explains that from `hello`, so a failure is no error line.
    fn receive_over_ble(&mut self) {
        self.ble_receiver = None;
        match (self.receive_ble)(self.id, &self.me(), self.ble_events.clone()) {
            Ok(receiver) => self.ble_receiver = Some(receiver),
            Err(e) => log::warn!("quickshare: not visible over Bluetooth, phones searching only there won't find us: {e:#}"),
        }
    }

    /// (Re)announces the current name and port.
    fn advertise(&mut self) {
        self.advertisement = None;
        let Some(daemon) = &self.daemon else { return };
        match mdns::advertise(daemon, self.id, &self.me(), self.listener.port) {
            Ok(a) => self.advertisement = Some(a),
            Err(e) => self.error(format!("{e:#}"), json!({})),
        }
    }

    /// Handles one message; false ends serve.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Eof => {
                log::info!("quickshare: stdin closed, ending");
                return false;
            }
            Msg::Stop => {
                log::info!("quickshare: signal received, ending");
                return false;
            }
            Msg::Bad(v) => {
                self.emit(&v);
            }
            Msg::Cmd(cmd) => self.command(cmd),
            Msg::Connection(stream) => self.incoming(stream),
            Msg::Event(e) => self.event(e),
            Msg::Ended(id) => self.ended(id),
            Msg::Peer(search, e) if Some(search) == self.search => self.peer(e),
            Msg::Peer(..) => {}
            Msg::PickDone => self.picking = false,
            Msg::Reply(v) => {
                self.emit(&v);
            }
        }
        self.connected
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Discover { on } => self.discover(on),
            Cmd::Send { device, files } => self.send(device, files),
            Cmd::Accept { id, dir } => self.accept(id, dir),
            Cmd::Reject { id } => {
                if let Some(decide) = self.take_decision(id) {
                    log::info!("quickshare: {id} rejected");
                    let _ = decide.send(Decision::Reject);
                }
            }
            Cmd::Cancel { id } => match self.transfers.get(&id) {
                Some(t) => {
                    log::info!("quickshare: cancelling {id}");
                    t.cancel.store(true, Ordering::SeqCst);
                }
                None => self.error(format!("no transfer {id}"), json!({ "id": id })),
            },
            Cmd::Dirs { req, dir } => {
                let reply = dirs_reply(req, &dir, &files::download_dir());
                self.emit(&reply);
            }
            Cmd::Mkdir { req, parent, name } => {
                let reply = mkdir_reply(req, &parent, &name);
                self.emit(&reply);
            }
            Cmd::Pick { req } => {
                if self.picking {
                    let busy = anyhow::anyhow!("a file dialog is already open");
                    self.emit(&pick_reply(req, Err(busy)));
                    return;
                }
                // The dialog stays open as long as the user likes.
                let tx = self.tx.clone();
                let spawned = std::thread::Builder::new().name("quickshare-pick".into()).spawn(move || {
                    let _ = tx.send(Msg::Reply(pick_reply(req, desktop::pick_files())));
                    let _ = tx.send(Msg::PickDone);
                });
                match spawned {
                    Ok(_) => self.picking = true,
                    Err(e) => {
                        self.emit(&pick_reply(req, Err(e.into())));
                    }
                }
            }
            Cmd::Stat { req, files } => {
                let reply = stat_reply(req, &files);
                self.emit(&reply);
            }
            Cmd::Reload => self.reload(),
        }
    }

    fn next_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// Runs `job` on its own thread with its own event channel; once the
    /// job is done and its events are passed on, `Ended` follows.
    fn spawn(
        &self,
        id: u64,
        job: impl FnOnce(mpsc::Sender<Event>, Arc<AtomicBool>) -> anyhow::Result<()> + Send + 'static,
    ) -> std::io::Result<(Arc<AtomicBool>, JoinHandle<()>)> {
        let cancel = Arc::new(AtomicBool::new(false));
        let (events, rx) = mpsc::channel();
        let tx = self.tx.clone();
        std::thread::Builder::new().name(format!("quickshare-{id}-events")).spawn(move || {
            for e in rx {
                if tx.send(Msg::Event(e)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Ended(id));
        })?;
        let flag = cancel.clone();
        let thread = std::thread::Builder::new().name(format!("quickshare-{id}")).spawn(move || {
            if let Err(e) = job(events, flag) {
                log::warn!("quickshare: transfer {id}: {e:#}");
            }
        })?;
        Ok((cancel, thread))
    }

    fn incoming(&mut self, stream: TcpStream) {
        let id = self.next_id();
        let from = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        log::info!("quickshare: connection {id} from {from}");
        let (decide, decisions) = mpsc::channel();
        match self.spawn(id, move |events, cancel| inbound::handle(stream, id, events, decisions, cancel)) {
            Ok((cancel, thread)) => {
                let mut t = Transfer::new("in", cancel, thread);
                t.decisions = Some(decide);
                self.transfers.insert(id, t);
            }
            Err(e) => log::warn!("quickshare: cannot handle connection {id}: {e}"),
        }
    }

    fn send(&mut self, device: String, paths: Vec<PathBuf>) {
        let Some(peer) = self.peers.get(&device).cloned() else {
            self.error(format!("no device {device} nearby"), json!({ "device": device }));
            return;
        };
        let id = self.next_id();
        let (me, own) = (self.me(), self.id);
        let total = paths.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum();
        log::info!("quickshare: sending {} file(s) to {:?} at {} as {id}", paths.len(), peer.name, peer.addr);
        let addr = peer.addr;
        match self.spawn(id, move |events, cancel| outbound::send(addr, &me, own, &paths, id, events, cancel)) {
            Ok((cancel, thread)) => {
                let mut t = Transfer::new("out", cancel, thread);
                // The shell names a device without a name by its kind.
                t.device = peer.name.unwrap_or_default();
                t.total = total;
                t.announced = true;
                let line = t.line(id);
                self.transfers.insert(id, t);
                self.emit(&line);
            }
            Err(e) => self.error(format!("cannot send: {e}"), json!({ "device": device })),
        }
    }

    /// The decision channel of an offer waiting for the user (taken: one
    /// decision per offer), else an error line.
    fn take_decision(&mut self, id: u64) -> Option<mpsc::Sender<Decision>> {
        let decide = self.transfers.get_mut(&id).filter(|t| t.announced).and_then(|t| t.decisions.take());
        if decide.is_none() {
            self.error(format!("no offer {id} is waiting"), json!({ "id": id }));
        }
        decide
    }

    fn accept(&mut self, id: u64, dir: String) {
        let Some(decide) = self.take_decision(id) else { return };
        let dir = if dir.is_empty() {
            let d = files::download_dir();
            let _ = fs::create_dir_all(&d);
            d
        } else {
            PathBuf::from(dir)
        };
        let Some(t) = self.transfers.get_mut(&id) else { return };
        if !dir.is_dir() {
            // The offer keeps waiting, so another folder can be chosen.
            t.decisions = Some(decide);
            self.error(format!("{} is not a folder", dir.display()), json!({ "id": id }));
            return;
        }
        log::info!("quickshare: {id} accepted into {}", dir.display());
        let _ = decide.send(Decision::Accept(dir.clone()));
        t.dir = lossy(&dir);
        t.state = "transferring";
        let line = t.line(id);
        self.emit(&line);
    }

    fn event(&mut self, e: Event) {
        let line = match e {
            Event::Incoming(offer) => {
                let Some(t) = self.transfers.get_mut(&offer.id) else { return };
                log::info!(
                    "quickshare: {} offers {} file(s) as {}, PIN {}",
                    offer.device,
                    offer.files.len(),
                    offer.id,
                    offer.pin
                );
                t.device = offer.device.clone();
                t.pin = offer.pin.clone();
                t.total = offer.total;
                t.announced = true;
                let line = t.line(offer.id);
                self.emit(&incoming_line(&offer));
                line
            }
            Event::Pin { id, pin } => {
                let Some(t) = self.transfers.get_mut(&id) else { return };
                t.pin = pin;
                t.line(id)
            }
            Event::Progress { id, done, total } => {
                let Some(t) = self.transfers.get_mut(&id) else { return };
                (t.done, t.total, t.state) = (done, total, "transferring");
                t.line(id)
            }
            Event::Text { id, kind, text } => {
                if let Some(t) = self.transfers.get(&id).filter(|t| t.direction == "in") {
                    let notice = text_notice(desktop::Lang::from_env(), kind, &text, &t.device);
                    (self.notify)(notice);
                }
                json!({ "type": "text", "id": id, "kind": kind, "text": text })
            }
            Event::Finished { id, outcome, saved } => {
                let Some(t) = self.transfers.get_mut(&id) else { return };
                if t.direction == "in" && outcome == Outcome::Done && !saved.is_empty() {
                    let notice = received_notice(desktop::Lang::from_env(), saved.len(), &t.device, &t.dir);
                    (self.notify)(notice);
                }
                log::info!("quickshare: {id} ended: {outcome:?}");
                (t.state, t.error) = state_of(&outcome);
                t.saved = saved.iter().map(|p| lossy(p)).collect();
                t.decisions = None;
                t.line(id)
            }
        };
        self.emit(&line);
    }

    fn ended(&mut self, id: u64) {
        if let Some(thread) = self.transfers.remove(&id).and_then(|mut t| t.thread.take()) {
            let _ = thread.join();
        }
    }

    fn discover(&mut self, on: bool) {
        let Some(daemon) = &self.daemon else {
            self.error("searching needs mDNS, which is not available".into(), json!({}));
            return;
        };
        if on && self.search.is_none() {
            match mdns::browse(daemon, self.id) {
                Ok(found) => {
                    self.searches += 1;
                    let (search, tx) = (self.searches, self.tx.clone());
                    std::thread::spawn(move || {
                        for e in found {
                            if tx.send(Msg::Peer(search, e)).is_err() {
                                return;
                            }
                        }
                    });
                    self.search = Some(search);
                    log::info!("quickshare: looking for devices");
                    self.start_beacon();
                }
                Err(e) => self.error(format!("{e:#}"), json!({})),
            }
        } else if !on && self.search.take().is_some() {
            let _ = daemon.stop_browse(SERVICE_TYPE);
            self.peers.clear();
            self.beacon = None;
            log::info!("quickshare: stopped looking for devices");
        }
        self.devices();
    }

    /// Without the beacon phones may not show up; the shell explains that
    /// from `hello`, so a failure is no error line.
    fn start_beacon(&mut self) {
        match ble::start() {
            Ok(beacon) => {
                self.beacon = Some(beacon);
                if !self.ble {
                    self.ble = true;
                    self.hello();
                }
            }
            Err(e) => {
                log::warn!("quickshare: no BLE beacon, phones may not show up: {e:#}");
                if self.ble {
                    self.ble = false;
                    self.hello();
                }
            }
        }
    }

    fn peer(&mut self, e: PeerEvent) {
        let changed = match e {
            PeerEvent::Up(p) if self.peers.get(&p.endpoint) != Some(&p) => {
                log::info!("quickshare: found {:?} ({}) at {}", p.name, p.endpoint, p.addr);
                self.peers.insert(p.endpoint.clone(), p);
                true
            }
            PeerEvent::Up(_) => false,
            PeerEvent::Down(id) => self.peers.remove(&id).is_some(),
        };
        if changed {
            self.devices();
        }
    }

    fn devices(&mut self) {
        let line = devices_line(&self.peers);
        self.emit(&line);
    }

    fn reload(&mut self) {
        let settings = (self.load)();
        let name = settings.effective_name();
        let renamed = name != self.name;
        let mut changed = renamed;
        if settings.port != self.configured_port {
            match Listener::bind(settings.port, self.tx.clone()) {
                Ok(l) => {
                    self.listener = l;
                    self.configured_port = settings.port;
                    changed = true;
                }
                Err(e) => self.error(format!("cannot listen on port {}: {e}", settings.port), json!({})),
            }
        }
        self.name = name;
        if changed {
            log::info!("quickshare: now {:?} on port {}", self.name, self.listener.port);
            self.advertise();
        }
        // Also when Bluetooth was not there before.
        if renamed || self.ble_receiver.is_none() {
            self.receive_over_ble();
        }
        self.ble = (self.probe_ble)();
        self.hello();
    }

    /// Withdraws the announcements, stops searching, the beacon and
    /// listening, cancels every transfer and waits a little for them to end.
    fn shutdown(mut self, rx: &mpsc::Receiver<Msg>) {
        self.advertisement = None;
        self.ble_receiver = None;
        self.beacon = None;
        if let (Some(daemon), Some(_)) = (&self.daemon, self.search.take()) {
            let _ = daemon.stop_browse(SERVICE_TYPE);
        }
        self.listener.stop();
        for t in self.transfers.values() {
            t.cancel.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !self.transfers.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                log::warn!("quickshare: {} transfer(s) did not end in time", self.transfers.len());
                break;
            };
            match rx.recv_timeout(left) {
                Ok(Msg::Event(e)) => self.event(e),
                Ok(Msg::Ended(id)) => self.ended(id),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if let Some(daemon) = self.daemon.take()
            && let Ok(done) = daemon.shutdown()
        {
            let _ = done.recv_timeout(MDNS_WAIT);
        }
    }
}

/// The write end of the pipe the signal handler writes to.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    // SAFETY: write(2) is async-signal-safe; the buffer is static.
    unsafe {
        libc::write(fd, b"x".as_ptr().cast(), 1);
    }
}

/// SIGTERM and SIGINT (also the parent's death, see `Lifecycle`) end serve
/// like a closed stdin; a second one ends the process at once.
fn forward_signals(tx: mpsc::Sender<Msg>) -> std::io::Result<()> {
    let mut fds = [0; 2];
    // SAFETY: an out-pointer to two ints.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a new descriptor nothing else owns.
    let mut pipe = unsafe { fs::File::from_raw_fd(fds[0]) };
    SIGNAL_PIPE.store(fds[1], Ordering::Relaxed);
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: an all-zero sigaction is valid; the handler only writes to the pipe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    std::thread::Builder::new().name("quickshare-signals".into()).spawn(move || {
        let mut byte = [0u8];
        if matches!(pipe.read(&mut byte), Ok(1)) {
            let _ = tx.send(Msg::Stop);
        }
        if matches!(pipe.read(&mut byte), Ok(1)) {
            log::warn!("quickshare: second signal, ending at once");
            std::process::exit(1);
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quickshare::endpoint::{DeviceKind, EndpointInfo};
    use crate::quickshare::outbound;
    use std::fs;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn every_command_of_the_protocol_parses() {
        let cases: Vec<(&str, Cmd)> = vec![
            (r#"{"cmd":"discover","on":true}"#, Cmd::Discover { on: true }),
            (r#"{"cmd":"discover","on":false}"#, Cmd::Discover { on: false }),
            (
                r#"{"cmd":"send","device":"AB12","files":["/abs/a.jpg"]}"#,
                Cmd::Send {
                    device: "AB12".into(),
                    files: vec!["/abs/a.jpg".into()],
                },
            ),
            (r#"{"cmd":"accept","id":7,"dir":"/abs"}"#, Cmd::Accept { id: 7, dir: "/abs".into() }),
            (r#"{"cmd":"reject","id":7}"#, Cmd::Reject { id: 7 }),
            (r#"{"cmd":"cancel","id":7}"#, Cmd::Cancel { id: 7 }),
            (
                r#"{"cmd":"dirs","req":3,"dir":"/abs"}"#,
                Cmd::Dirs {
                    req: Some(3),
                    dir: "/abs".into(),
                },
            ),
            (r#"{"cmd":"dirs","dir":""}"#, Cmd::Dirs { req: None, dir: String::new() }),
            (
                r#"{"cmd":"mkdir","req":4,"parent":"/abs","name":"Neu"}"#,
                Cmd::Mkdir {
                    req: Some(4),
                    parent: "/abs".into(),
                    name: "Neu".into(),
                },
            ),
            (r#"{"cmd":"pick","req":5}"#, Cmd::Pick { req: Some(5) }),
            (
                r#"{"cmd":"stat","req":6,"files":["/abs"]}"#,
                Cmd::Stat {
                    req: Some(6),
                    files: vec!["/abs".into()],
                },
            ),
            (r#"{"cmd":"reload"}"#, Cmd::Reload),
        ];
        for (line, want) in cases {
            assert_eq!(parse_cmd(line).unwrap(), want, "{line}");
        }
    }

    #[test]
    fn bad_commands_are_refused() {
        for line in [
            r#"{"cmd":"explode"}"#,
            r#"{"cmd":"accept"}"#,
            r#"{"cmd":"accept","id":"x"}"#,
            "not json",
            r#"{"on":true}"#,
        ] {
            assert!(parse_cmd(line).is_err(), "{line}");
        }
    }

    #[test]
    fn a_bad_line_is_answered_with_an_error_and_reading_goes_on() {
        let (tx, rx) = mpsc::channel();
        let input = "{\"cmd\":\"explode\",\"req\":9}\n\n{\"cmd\":\"reload\"}\n";
        read_commands(input.as_bytes(), &tx);
        let Msg::Bad(err) = rx.recv().unwrap() else {
            panic!("expected an error line")
        };
        assert_eq!(err["type"], "error");
        assert_eq!(err["req"], 9);
        assert!(err["message"].as_str().unwrap().contains("explode"), "{err}");
        assert!(matches!(rx.recv().unwrap(), Msg::Cmd(Cmd::Reload)));
        assert!(matches!(rx.recv().unwrap(), Msg::Eof));
    }

    #[test]
    fn outcomes_become_transfer_states() {
        assert_eq!(state_of(&Outcome::Done), ("done", String::new()));
        assert_eq!(state_of(&Outcome::Rejected), ("rejected", String::new()));
        assert_eq!(state_of(&Outcome::Cancelled), ("cancelled", String::new()));
        assert_eq!(state_of(&Outcome::NotEnoughSpace), ("no-space", String::new()));
        assert_eq!(state_of(&Outcome::Failed("boom".into())), ("failed", "boom".into()));
    }

    #[test]
    fn dirs_lists_subfolders_and_defaults_to_the_download_folder() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("Busan")).unwrap();
        fs::create_dir(tmp.path().join("Campus")).unwrap();
        fs::write(tmp.path().join("file.txt"), "x").unwrap();
        let dir = tmp.path().to_str().unwrap();
        let r = dirs_reply(Some(3), dir, Path::new("/unused"));
        assert_eq!(r["type"], "dirs");
        assert_eq!(r["req"], 3);
        assert_eq!(r["dir"], dir);
        assert_eq!(r["parent"], tmp.path().parent().unwrap().to_str().unwrap());
        assert_eq!(r["dirs"], json!(["Busan", "Campus"]));
        assert_eq!(r["error"], "");

        let downloads = tmp.path().join("home/Downloads");
        let r = dirs_reply(None, "", &downloads);
        assert!(downloads.is_dir(), "created when missing");
        assert_eq!(r["dir"], downloads.to_str().unwrap());
        assert_eq!(r["req"], Value::Null);
        assert_eq!(r["dirs"], json!([]));

        let r = dirs_reply(Some(1), tmp.path().join("gone").to_str().unwrap(), &downloads);
        assert_ne!(r["error"], "");
        assert_eq!(r["dirs"], json!([]));
        assert_eq!(dirs_reply(None, "/", &downloads)["parent"], "");
    }

    #[test]
    fn mkdir_stat_and_pick_replies() {
        let tmp = tempfile::tempdir().unwrap();
        let r = mkdir_reply(Some(4), tmp.path(), "Neu");
        assert_eq!(r["type"], "mkdir");
        assert_eq!(r["req"], 4);
        assert_eq!(r["path"], tmp.path().join("Neu").to_str().unwrap());
        assert_eq!(r["error"], "");
        assert!(tmp.path().join("Neu").is_dir());
        assert_ne!(mkdir_reply(None, tmp.path(), "Neu")["error"], "", "exists already");
        assert_ne!(mkdir_reply(None, tmp.path(), "../x")["error"], "");

        let file = tmp.path().join("a.jpg");
        fs::write(&file, b"12345").unwrap();
        let r = stat_reply(Some(6), &[file.clone(), tmp.path().join("Neu"), tmp.path().join("gone")]);
        assert_eq!(r["type"], "picked");
        assert_eq!(r["req"], 6);
        assert_eq!(r["files"], json!([{ "path": file.to_str().unwrap(), "name": "a.jpg", "size": 5 }]));
        let error = r["error"].as_str().unwrap();
        assert!(error.contains("Neu") && error.contains("gone"), "{error}");

        let r = pick_reply(Some(5), Ok(vec![file.clone()]));
        assert_eq!(r["files"], json!([{ "path": file.to_str().unwrap(), "name": "a.jpg", "size": 5 }]));
        let r = pick_reply(Some(5), Ok(Vec::new()));
        assert_eq!(
            (&r["type"], &r["req"], &r["files"], &r["error"]),
            (&json!("picked"), &json!(5), &json!([]), &json!("")),
            "cancelled"
        );
        let r = pick_reply(Some(5), Err(anyhow::anyhow!(desktop::NO_DIALOG)));
        assert_eq!((&r["files"], &r["error"]), (&json!([]), &json!(desktop::NO_DIALOG)));
    }

    #[test]
    fn text_and_file_notices() {
        use desktop::Lang;
        let n = text_notice(Lang::En, "url", "https://example.org/x", "Pixel");
        assert_eq!((n.summary.as_str(), n.body.as_str()), ("https://example.org/x", "Pixel"));
        assert_eq!(n.action, Some(("Open".into(), "https://example.org/x".into())));
        assert_eq!(text_notice(Lang::De, "url", "ftp://x", "Pixel").action, None, "only web links open");
        let n = text_notice(Lang::De, "text", &"y".repeat(200), "Pixel");
        assert_eq!(n.summary, "Text von Pixel kopiert");
        assert_eq!(n.body.chars().count(), 80);
        assert_eq!(n.action, None);
        let n = received_notice(Lang::De, 3, "Pixel", "/home/a/Downloads");
        assert_eq!((n.summary.as_str(), n.body.as_str()), ("3 Dateien empfangen", "Pixel · /home/a/Downloads"));
        assert_eq!(n.action, Some(("Ordner öffnen".into(), "file:///home/a/Downloads".into())));
    }

    #[test]
    fn devices_lists_named_peers_first_and_nameless_ones_with_null() {
        let peer = |endpoint: &str, name: Option<&str>| {
            let p = mdns::Peer {
                endpoint: endpoint.into(),
                name: name.map(str::to_owned),
                kind: DeviceKind::Phone,
                addr: "192.168.0.51:53601".parse().unwrap(),
            };
            (p.endpoint.clone(), p)
        };
        let peers = BTreeMap::from([peer("VCRE", None), peer("AB12", None), peer("ZZZZ", Some("anna")), peer("CCCC", Some("Bob"))]);
        assert_eq!(
            devices_line(&peers),
            json!({ "type": "devices", "devices": [
                { "id": "ZZZZ", "name": "anna", "kind": "phone" },
                { "id": "CCCC", "name": "Bob", "kind": "phone" },
                { "id": "AB12", "name": null, "kind": "phone" },
                { "id": "VCRE", "name": null, "kind": "phone" },
            ]})
        );
    }

    /// Collects the JSON lines `serve` writes.
    struct Lines(mpsc::Sender<Value>, Vec<u8>);

    impl Write for Lines {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.1.extend_from_slice(b);
            while let Some(i) = self.1.iter().position(|c| *c == b'\n') {
                let line: Vec<u8> = self.1.drain(..=i).collect();
                let _ = self.0.send(serde_json::from_slice(&line).expect("one JSON document per line"));
            }
            Ok(b.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Running {
        notices: mpsc::Receiver<Notice>,
        tx: mpsc::Sender<Msg>,
        lines: mpsc::Receiver<Value>,
        thread: std::thread::JoinHandle<anyhow::Result<()>>,
    }

    fn start(port: u16) -> Running {
        start_with(port, |_| {})
    }

    /// Like [`start`], with `probe_ble` saying whether BLE can run.
    fn start_with_ble(port: u16, probe_ble: Box<dyn Fn() -> bool + Send>) -> Running {
        start_with(port, |o| o.probe_ble = probe_ble)
    }

    /// Like [`start`], with options changed by `adjust`. By default the
    /// beacon cannot run and receiving over Bluetooth fails (the host's
    /// Bluetooth is never asked).
    fn start_with(port: u16, adjust: impl FnOnce(&mut Options)) -> Running {
        let (tx, rx) = mpsc::channel();
        let (ltx, lines) = mpsc::channel();
        let (ntx, notices) = mpsc::channel();
        let mut opts = Options {
            mdns: false,
            load: Box::new(move || config::Quickshare { name: "test-pc".into(), port }),
            probe_ble: Box::new(|| false),
            receive_ble: Box::new(|_, _, _| Err(anyhow::anyhow!("no Bluetooth in tests"))),
            notify: Box::new(move |n| {
                let _ = ntx.send(n);
            }),
        };
        adjust(&mut opts);
        let serve_tx = tx.clone();
        let thread = std::thread::spawn(move || serve(serve_tx, rx, &mut Lines(ltx, Vec::new()), opts));
        Running { notices, tx, lines, thread }
    }

    /// The next line of type `ty`, skipping the others.
    fn next(lines: &mpsc::Receiver<Value>, ty: &str) -> Value {
        loop {
            let v = lines.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| panic!("no {ty} line"));
            if v["type"] == ty {
                return v;
            }
        }
    }

    /// The next `transfer` line of `id` in one of the final states.
    fn finished(lines: &mpsc::Receiver<Value>, id: &Value) -> Value {
        loop {
            let v = next(lines, "transfer");
            if &v["id"] == id && !["waiting", "transferring"].contains(&v["state"].as_str().unwrap()) {
                return v;
            }
        }
    }

    struct Peer {
        events: mpsc::Receiver<Event>,
        thread: std::thread::JoinHandle<anyhow::Result<()>>,
    }

    /// A phone sending `files` to `addr`.
    fn phone(addr: SocketAddr, files: &[PathBuf]) -> Peer {
        let (tx, events) = mpsc::channel();
        let files = files.to_vec();
        let me = EndpointInfo {
            kind: DeviceKind::Phone,
            name: Some("Galaxy S24".into()),
        };
        let thread = std::thread::spawn(move || outbound::send(addr, &me, *b"PHON", &files, 1, tx, Arc::new(AtomicBool::new(false))));
        Peer { events, thread }
    }

    fn outcome(p: Peer) -> Outcome {
        let _ = p.thread.join();
        p.events
            .try_iter()
            .find_map(|e| match e {
                Event::Finished { outcome, .. } => Some(outcome),
                _ => None,
            })
            .expect("the phone got an outcome")
    }

    fn hello(r: &Running) -> SocketAddr {
        let hello = next(&r.lines, "hello");
        assert_eq!(hello["name"], "test-pc");
        assert_eq!(hello["ble"], false);
        assert_eq!(hello["bleReceive"], false);
        SocketAddr::from(([127, 0, 0, 1], hello["port"].as_u64().unwrap() as u16))
    }

    #[test]
    fn an_incoming_file_is_saved_after_accepting_it() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();
        let r = start(0);
        let addr = hello(&r);

        let p = phone(addr, std::slice::from_ref(&file));
        let inc = next(&r.lines, "incoming");
        assert_eq!(inc["device"], "Galaxy S24");
        assert_eq!(inc["kind"], "phone");
        assert_eq!(inc["files"], json!([{ "name": "a.txt", "size": 5, "mime": "text/plain" }]));
        assert_eq!(inc["text"], Value::Null);
        assert_eq!(inc["total"], 5);
        assert_eq!(inc["pin"].as_str().unwrap().len(), 4);
        let id = inc["id"].clone();
        let waiting = next(&r.lines, "transfer");
        assert_eq!(
            (&waiting["id"], &waiting["direction"], &waiting["state"]),
            (&id, &json!("in"), &json!("waiting"))
        );
        assert_eq!(waiting["pin"], inc["pin"]);

        let n = id.as_u64().unwrap();
        r.tx.send(Msg::Cmd(Cmd::Accept {
            id: n + 100,
            dir: String::new(),
        }))
        .unwrap();
        assert_ne!(next(&r.lines, "error")["message"], "", "unknown id");
        let gone = dest.path().join("gone");
        r.tx.send(Msg::Cmd(Cmd::Accept {
            id: n,
            dir: gone.to_str().unwrap().into(),
        }))
        .unwrap();
        let err = next(&r.lines, "error");
        assert_eq!(err["id"], id, "a missing folder keeps the offer waiting");

        r.tx.send(Msg::Cmd(Cmd::Accept {
            id: n,
            dir: dest.path().to_str().unwrap().into(),
        }))
        .unwrap();
        let done = finished(&r.lines, &id);
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["done"], 5);
        assert_eq!(done["dir"], dest.path().to_str().unwrap());
        let saved = dest.path().join("a.txt");
        assert_eq!(done["saved"], json!([saved.to_str().unwrap()]));
        assert_eq!(fs::read(&saved).unwrap(), b"hello");
        assert_eq!(outcome(p), Outcome::Done);
        let notice = r.notices.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(notice.body, format!("Galaxy S24 · {}", dest.path().display()));
        assert_eq!(notice.action.unwrap().1, desktop::file_uri(dest.path().to_str().unwrap()));
        assert!(r.notices.try_recv().is_err(), "one notification per transfer");

        r.tx.send(Msg::Eof).unwrap();
        r.thread.join().unwrap().unwrap();
    }

    #[test]
    fn offers_wait_side_by_side_and_end_on_reject_and_on_stop() {
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();
        let r = start(0);
        let addr = hello(&r);

        let first = phone(addr, std::slice::from_ref(&file));
        let a = next(&r.lines, "incoming")["id"].clone();
        let second = phone(addr, std::slice::from_ref(&file));
        let b = next(&r.lines, "incoming")["id"].clone();
        assert_ne!(a, b);

        r.tx.send(Msg::Cmd(Cmd::Reject { id: a.as_u64().unwrap() })).unwrap();
        assert_eq!(finished(&r.lines, &a)["state"], "rejected");
        assert_eq!(outcome(first), Outcome::Rejected);

        r.tx.send(Msg::Cmd(Cmd::Send {
            device: "NONE".into(),
            files: vec![file.clone()],
        }))
        .unwrap();
        assert!(next(&r.lines, "error")["message"].as_str().unwrap().contains("NONE"));

        // SIGTERM: what is still waiting is cancelled before serve ends.
        r.tx.send(Msg::Stop).unwrap();
        assert_eq!(finished(&r.lines, &b)["state"], "cancelled");
        r.thread.join().unwrap().unwrap();
        assert_eq!(outcome(second), Outcome::Cancelled);
    }

    #[test]
    fn hello_says_whether_ble_can_run_and_reload_asks_again() {
        let powered = Arc::new(AtomicBool::new(true));
        let probe = powered.clone();
        let r = start_with_ble(0, Box::new(move || probe.load(Ordering::SeqCst)));
        assert_eq!(next(&r.lines, "hello")["ble"], true);

        powered.store(false, Ordering::SeqCst);
        r.tx.send(Msg::Cmd(Cmd::Reload)).unwrap();
        assert_eq!(next(&r.lines, "hello")["ble"], false);

        r.tx.send(Msg::Eof).unwrap();
        r.thread.join().unwrap().unwrap();
    }

    /// Counts its drops.
    struct Guard(Arc<AtomicUsize>);

    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn visible_over_ble_while_serving_and_under_a_new_name() {
        let name = Arc::new(Mutex::new("test-pc".to_owned()));
        let (started, stopped) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
        let (n, s, d) = (name.clone(), started.clone(), stopped.clone());
        let r = start_with(0, move |o| {
            o.load = Box::new(move || config::Quickshare {
                name: n.lock().unwrap().clone(),
                port: 0,
            });
            o.receive_ble = Box::new(move |id, info, _| {
                s.lock().unwrap().push((id, info.name.clone().unwrap()));
                Ok(Box::new(Guard(d.clone())))
            });
        });
        assert_eq!(next(&r.lines, "hello")["bleReceive"], true);

        r.tx.send(Msg::Cmd(Cmd::Reload)).unwrap();
        assert_eq!(next(&r.lines, "hello")["bleReceive"], true);
        assert_eq!(started.lock().unwrap().len(), 1, "the same name keeps the receiver");

        *name.lock().unwrap() = "renamed".into();
        r.tx.send(Msg::Cmd(Cmd::Reload)).unwrap();
        assert_eq!(next(&r.lines, "hello")["bleReceive"], true);
        assert_eq!(stopped.load(Ordering::SeqCst), 1, "the old receiver is gone");

        r.tx.send(Msg::Eof).unwrap();
        r.thread.join().unwrap().unwrap();
        assert_eq!(stopped.load(Ordering::SeqCst), 2, "ending stops the receiver");
        let started = started.lock().unwrap();
        let names: Vec<&str> = started.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["test-pc", "renamed"]);
        assert_eq!(started[0].0, started[1].0, "one endpoint id per run");
    }

    #[test]
    fn a_port_in_use_is_an_error() {
        let taken = TcpListener::bind("0.0.0.0:0").unwrap();
        let r = start(taken.local_addr().unwrap().port());
        let err = next(&r.lines, "error");
        assert!(
            err["message"].as_str().unwrap().contains(&taken.local_addr().unwrap().port().to_string()),
            "{err}"
        );
        assert!(r.thread.join().unwrap().is_err());
    }
}
