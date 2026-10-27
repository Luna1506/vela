//! Desktop services for Quick Share: notifications, opening links and
//! folders, and the file dialog for sending. Everything talks to the session
//! bus (or to `kdialog`/`zenity`) and is bounded, so a missing or hung
//! notification daemon or portal cannot block the serve loop; the one call
//! that waits on the user, [`pick_files`], is meant for a worker thread.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, bail};
use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{MatchRule, names::BusName};

/// The longest any one D-Bus call may take (not counting the wait for the
/// user in the file dialog).
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// What the file dialog fallbacks say when nothing can show a dialog.
pub const NO_DIALOG: &str = "Kein Dateiauswahl-Dialog verfügbar";

// ---- Texts (the notifications come from Rust, not from the shell) ----

/// The languages of the notification texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

impl Lang {
    /// German when the first non-empty of `LC_ALL`, `LC_MESSAGES`, `LANG`
    /// (POSIX precedence) starts with `de`, else English.
    pub fn from_env() -> Lang {
        Lang::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(get: impl Fn(&str) -> Option<String>) -> Lang {
        let value = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|n| get(n)).find(|v| !v.is_empty());
        match value {
            Some(v) if v.starts_with("de") => Lang::De,
            _ => Lang::En,
        }
    }
}

/// "3 Dateien empfangen" / "1 file received".
pub fn received_summary(lang: Lang, n: usize) -> String {
    match (lang, n) {
        (Lang::De, 1) => "1 Datei empfangen".into(),
        (Lang::De, n) => format!("{n} Dateien empfangen"),
        (Lang::En, 1) => "1 file received".into(),
        (Lang::En, n) => format!("{n} files received"),
    }
}

pub fn open_folder_label(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Ordner öffnen",
        Lang::En => "Open folder",
    }
}

pub fn open_label(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Öffnen",
        Lang::En => "Open",
    }
}

/// "Text von Pixel kopiert" / "Text from Pixel copied".
pub fn text_copied_summary(lang: Lang, device: &str) -> String {
    match lang {
        Lang::De => format!("Text von {device} kopiert"),
        Lang::En => format!("Text from {device} copied"),
    }
}

fn dialog_title(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Dateien für Quick Share",
        Lang::En => "Files for Quick Share",
    }
}

/// The first `max` characters of `text`, with "…" when cut, on one line.
pub fn preview(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

// ---- URIs ----

/// A `file://` URI for an absolute path.
pub fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(b as char),
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

/// The path of a local `file://` URI (percent-decoded); `None` for anything
/// else.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = match rest.split_once('/') {
        Some(("" | "localhost", path)) => path,
        _ => return None,
    };
    let bytes = format!("/{path}").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(out)))
}

/// Opens a URI (a link or a folder) with `xdg-open`, else `gio open`.
pub fn open_uri(uri: &str) {
    for (program, args) in [("xdg-open", &[][..]), ("gio", &["open"][..])] {
        match Command::new(program)
            .args(args)
            .arg(uri)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Reap it without waiting here.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return;
            }
            Err(e) => log::debug!("quickshare: {program}: {e}"),
        }
    }
    log::warn!("quickshare: cannot open {uri}: neither xdg-open nor gio is available");
}

// ---- Notifications ----

/// What to show: a summary, a body and optionally one action (label, URI to
/// open when it is chosen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    pub action: Option<(String, String)>,
}

/// The shared notification connection and the actions it waits for.
struct Listener {
    conn: Connection,
    /// Notification id → URI to open.
    actions: HashMap<u32, String>,
}

static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

fn listener() -> MutexGuard<'static, Option<Listener>> {
    LISTENER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The session bus connection, with the background thread that listens for
/// `ActionInvoked`/`NotificationClosed` started on first use. The match rule
/// is added before this returns, so no action can be missed.
fn notification_bus() -> anyhow::Result<Connection> {
    if let Some(l) = listener().as_ref() {
        return Ok(l.conn.clone());
    }
    let conn = Builder::session()
        .and_then(|b| b.method_timeout(CALL_TIMEOUT).build())
        .context("the session D-Bus is not available")?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(NOTIFICATIONS)
        .context("match rule")?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &conn, None).context("listening for notification actions")?;
    *listener() = Some(Listener {
        conn: conn.clone(),
        actions: HashMap::new(),
    });
    let spawned = std::thread::Builder::new().name("quickshare-notify".into()).spawn(move || {
        for msg in signals.flatten() {
            let header = msg.header();
            let Some(member) = header.member() else { continue };
            match member.as_str() {
                "ActionInvoked" => {
                    let Ok((id, _key)) = msg.body().deserialize::<(u32, String)>() else {
                        continue;
                    };
                    let uri = listener().as_ref().and_then(|l| l.actions.get(&id).cloned());
                    if let Some(uri) = uri {
                        open_uri(&uri);
                    }
                }
                "NotificationClosed" => {
                    let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() else {
                        continue;
                    };
                    if let Some(l) = listener().as_mut() {
                        l.actions.remove(&id);
                    }
                }
                _ => {}
            }
        }
        // The connection ended; the next notification starts over.
        *listener() = None;
    });
    if let Err(e) = spawned {
        *listener() = None;
        bail!("cannot start the notification listener: {e}");
    }
    Ok(conn)
}

/// Shows a notification; `action` is a button (and the click on the
/// notification) that opens the URI. Bounded by [`CALL_TIMEOUT`].
pub fn notify(summary: &str, body: &str, action: Option<(&str, String)>) -> anyhow::Result<()> {
    let conn = notification_bus()?;
    let actions: Vec<&str> = action.iter().flat_map(|(label, _)| ["default", *label]).collect();
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let reply = conn
        .call_method(
            Some(NOTIFICATIONS),
            NOTIFICATIONS_PATH,
            Some(NOTIFICATIONS),
            "Notify",
            &("vela", 0u32, "folder-download", summary, body, actions, hints, -1i32),
        )
        .context("showing the notification")?;
    let id: u32 = reply.body().deserialize().context("reading the notification id")?;
    if let Some((_, uri)) = action
        && let Some(l) = listener().as_mut()
    {
        l.actions.insert(id, uri);
    }
    Ok(())
}

/// [`notify`] on its own thread; failures are only logged.
pub fn notify_detached(notice: Notice) {
    let spawned = std::thread::Builder::new().name("quickshare-notify-send".into()).spawn(move || {
        let action = notice.action.as_ref().map(|(label, uri)| (label.as_str(), uri.clone()));
        if let Err(e) = notify(&notice.summary, &notice.body, action) {
            log::info!("quickshare: no notification: {e:#}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("quickshare: cannot start a notification thread: {e}");
    }
}

// ---- File dialog ----

/// Lets the user pick files: the FileChooser portal, else `kdialog`, else
/// `zenity`. Waits for as long as the dialog is open; a cancelled dialog is
/// an empty list.
pub fn pick_files() -> anyhow::Result<Vec<PathBuf>> {
    let lang = Lang::from_env();
    match pick_with_portal(dialog_title(lang)) {
        Ok(paths) => return Ok(paths),
        Err(PortalError::Unavailable(e)) => log::info!("quickshare: no file chooser portal: {e:#}"),
        Err(PortalError::Failed(e)) => return Err(e),
    }
    for (program, args) in dialog_commands(dialog_title(lang)) {
        match run_dialog(program, &args)? {
            Some(paths) => return Ok(paths),
            None => continue,
        }
    }
    bail!(NO_DIALOG)
}

enum PortalError {
    /// The portal (or its FileChooser) is not there: try the fallbacks.
    Unavailable(anyhow::Error),
    /// The dialog was shown and something went wrong.
    Failed(anyhow::Error),
}

static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// The request object the portal answers on: `…/request/<sender>/<token>`,
/// where the sender is our unique name without `:` and with `_` for `.`.
fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

fn pick_with_portal(title: &str) -> Result<Vec<PathBuf>, PortalError> {
    let unavailable = |e: anyhow::Error| PortalError::Unavailable(e);
    let conn = Builder::session()
        .and_then(|b| b.method_timeout(CALL_TIMEOUT).build())
        .context("the session D-Bus is not available")
        .map_err(unavailable)?;
    let token = format!("vela_qs_{}_{}", std::process::id(), REQUESTS.fetch_add(1, Ordering::Relaxed));
    let unique = conn.unique_name().map(|n| n.to_string()).unwrap_or_default();
    let expected = request_path(&unique, &token);
    // Subscribe before the call: the response can come right away.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .and_then(|b| b.path(expected.as_str()))
        .map(|b| b.build())
        .context("match rule")
        .map_err(unavailable)?;
    let responses = MessageIterator::for_match_rule(rule, &conn, None)
        .context("listening for the portal")
        .map_err(unavailable)?;

    let options: HashMap<&str, Value<'_>> = HashMap::from([("multiple", Value::from(true)), ("handle_token", Value::from(token.as_str()))]);
    let bus = BusName::try_from(PORTAL).map_err(|e| unavailable(e.into()))?;
    let reply = conn
        .call_method(
            Some(bus),
            PORTAL_PATH,
            Some("org.freedesktop.portal.FileChooser"),
            "OpenFile",
            &("", title, options),
        )
        .context("opening the file chooser portal")
        .map_err(unavailable)?;
    let handle: zbus::zvariant::OwnedObjectPath = reply.body().deserialize().context("reading the portal request").map_err(unavailable)?;
    if handle.as_str() != expected {
        // The portal is older than the handle_token scheme: we may have
        // missed nothing, but we listen on the wrong path.
        return Err(PortalError::Failed(anyhow::anyhow!(
            "the portal answers on {} instead of {expected}",
            handle.as_str()
        )));
    }

    let Some(msg) = responses.flatten().next() else {
        return Err(PortalError::Failed(anyhow::anyhow!("the session D-Bus closed during the file dialog")));
    };
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().context("reading the portal response").map_err(PortalError::Failed)?;
    match code {
        0 => {
            let uris = results
                .get("uris")
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| Vec::<String>::try_from(v).ok())
                .unwrap_or_default();
            Ok(uris.iter().filter_map(|u| path_from_uri(u)).collect())
        }
        1 => Ok(Vec::new()),
        n => Err(PortalError::Failed(anyhow::anyhow!("the file dialog ended with code {n}"))),
    }
}

/// The fallback dialogs, in order of preference.
fn dialog_commands(title: &str) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "kdialog",
            ["--title", title, "--getopenfilename", "--multiple", "--separate-output"]
                .map(String::from)
                .to_vec(),
        ),
        (
            "zenity",
            vec![
                "--file-selection".into(),
                "--multiple".into(),
                "--separator=\n".into(),
                format!("--title={title}"),
            ],
        ),
    ]
}

/// Runs a dialog program: `None` when it is not installed, else the chosen
/// files (none when cancelled).
fn run_dialog(program: &str, args: &[String]) -> anyhow::Result<Option<Vec<PathBuf>>> {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("starting {program}")),
    };
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_string(&mut text);
        }
        text
    });
    let mut stdout = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout).with_context(|| format!("reading {program}"))?;
    }
    let status = child.wait().with_context(|| format!("waiting for {program}"))?;
    let stderr = reader.join().unwrap_or_default();
    let paths = parse_dialog_output(&stdout);
    // Both programs exit with 1 when the dialog is cancelled.
    if status.success() || (status.code() == Some(1) && paths.is_empty() && stderr.trim().is_empty()) {
        Ok(Some(paths))
    } else {
        bail!("{program} failed: {}", stderr.trim())
    }
}

/// One path per line; empty lines are ignored.
fn parse_dialog_output(out: &[u8]) -> Vec<PathBuf> {
    out.split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(OsString::from_vec(line.to_vec())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_text_de_en() {
        assert_eq!(received_summary(Lang::De, 1), "1 Datei empfangen");
        assert_eq!(received_summary(Lang::De, 3), "3 Dateien empfangen");
        assert_eq!(received_summary(Lang::En, 1), "1 file received");
        assert_eq!(received_summary(Lang::En, 3), "3 files received");
        assert_eq!(text_copied_summary(Lang::De, "Pixel"), "Text von Pixel kopiert");
        assert_eq!(text_copied_summary(Lang::En, "Pixel"), "Text from Pixel copied");
        assert_eq!(open_folder_label(Lang::De), "Ordner öffnen");
        assert_eq!(open_label(Lang::De), "Öffnen");
    }

    #[test]
    fn language_follows_posix_precedence() {
        let lang = |vars: &[(&str, &str)]| {
            let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            Lang::from_vars(|n| vars.get(n).cloned())
        };
        assert_eq!(lang(&[]), Lang::En);
        assert_eq!(lang(&[("LANG", "de_DE.UTF-8")]), Lang::De);
        assert_eq!(lang(&[("LANG", "en_US.UTF-8")]), Lang::En);
        assert_eq!(lang(&[("LANG", "de_DE.UTF-8"), ("LC_ALL", "en_US.UTF-8")]), Lang::En);
        assert_eq!(lang(&[("LANG", "en_US"), ("LC_MESSAGES", "de_AT")]), Lang::De);
        assert_eq!(lang(&[("LC_ALL", ""), ("LANG", "de")]), Lang::De, "empty values are skipped");
    }

    #[test]
    fn preview_is_one_line_and_short() {
        assert_eq!(preview("a\n  b", 80), "a b");
        let long = "x".repeat(100);
        let p = preview(&long, 80);
        assert_eq!(p.chars().count(), 80);
        assert!(p.ends_with('…'));
        assert_eq!(preview(&"ä".repeat(80), 80).chars().count(), 80);
    }

    #[test]
    fn zenity_output_parsing() {
        assert_eq!(
            parse_dialog_output(b"/home/a/one.txt\n\n/home/a/two words.png\n"),
            vec![PathBuf::from("/home/a/one.txt"), PathBuf::from("/home/a/two words.png")]
        );
        assert!(parse_dialog_output(b"").is_empty());
        assert!(parse_dialog_output(b"\n\n").is_empty());
    }

    #[test]
    fn uris_become_paths_and_back() {
        assert_eq!(path_from_uri("file:///home/a/b.txt"), Some(PathBuf::from("/home/a/b.txt")));
        assert_eq!(path_from_uri("file:///home/a/a%20b%C3%A4.txt"), Some(PathBuf::from("/home/a/a bä.txt")));
        assert_eq!(path_from_uri("file://localhost/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(path_from_uri("file://other/tmp/x"), None);
        assert_eq!(path_from_uri("https://example.org/x"), None);
        assert_eq!(path_from_uri("file:///100%"), Some(PathBuf::from("/100%")), "a stray % stays");
        for p in ["/home/a/a b#ä.txt", "/tmp/100%", "/x"] {
            assert_eq!(path_from_uri(&file_uri(p)), Some(PathBuf::from(p)), "{p}");
        }
        assert_eq!(file_uri("/a b"), "file:///a%20b");
    }

    #[test]
    fn portal_request_path_follows_the_spec() {
        assert_eq!(request_path(":1.42", "tok"), "/org/freedesktop/portal/desktop/request/1_42/tok");
    }
}
