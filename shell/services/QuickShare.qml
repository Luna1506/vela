pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io
import qs
import "../pulse/Fmt.js" as Fmt

// Quick Share with phones nearby. `vela quickshare serve` does the work
// (src/quickshare/serve.rs): JSON lines on stdout, commands as JSON lines on
// stdin. The process runs only while Quick Share is on and installed.
Singleton {
    id: root

    // Installed and switched on (the switch lives in Persist).
    readonly property bool enabled: Persist.quickShare && Config.quickShareAvailable

    property bool connected: false
    property bool ble: false
    // Visible name; the backend's answer wins over the configured one.
    property string name: Config.quickShareName
    // [{ id, name (null: announced without a name), kind }]
    property var devices: []
    // transfer id -> last `transfer` line
    property var transfers: ({})
    // Offers waiting for a decision, oldest first; the first one is shown.
    property var incoming: []
    // Files picked for sending: [{ path, name, size }]
    property var pendingFiles: []
    readonly property string lastDir: Persist.quickShareDir

    // Whether the shell wants to see devices (the send view is open).
    property bool discovering: false

    // `transfer` lines carry the device's name, not its id: device ids of
    // sends whose first line hasn't come yet, oldest first, and the device
    // each outgoing transfer went to (transfer id -> device id).
    property var sendQueue: []
    property var sentTo: ({})

    // Something went wrong the user should hear about.
    signal failed(string message)

    // A transfer in one of these states is over.
    readonly property var finalStates: ["done", "rejected", "cancelled", "failed", "no-space"]

    readonly property int maxFinished: 20

    property int nextReq: 1
    // req -> callback, for replies to dirs / mkdir / pick / stat.
    property var pending: ({})
    // The backend is down for a moment; `proc.running` waits for the retry.
    property bool backoff: false

    function toggle(): void {
        setEnabled(!Persist.quickShare);
    }

    function setEnabled(on: bool): void {
        Persist.quickShare = on;
    }

    function discover(on: bool): void {
        discovering = on;
        if (connected)
            send({
                cmd: "discover",
                on: on
            });
    }

    function addFiles(paths: var): void {
        if (paths.length === 0)
            return;
        request({
            cmd: "stat",
            files: paths
        }, root.mergePicked);
    }

    function removeFile(path: string): void {
        pendingFiles = pendingFiles.filter(f => f.path !== path);
    }

    function pickFiles(): void {
        request({
            cmd: "pick"
        }, root.mergePicked);
    }

    function sendTo(deviceId: string): void {
        if (pendingFiles.length === 0)
            return;
        if (!send({
            cmd: "send",
            device: deviceId,
            files: pendingFiles.map(f => f.path)
        }))
            failed(I18n.tr("Quick Share is not running"));
        else
            sendQueue = sendQueue.concat([deviceId]);
    }

    // The newest transfer started from here to this device, or null.
    function deviceTransfer(deviceId: string): var {
        let found = null;
        for (const id in sentTo)
            if (sentTo[id] === deviceId && transfers[id] && (!found || Number(id) > found.id))
                found = transfers[id];
        return found;
    }

    // What to call a device: its name, or for one announced without a name
    // (Samsung phones do that) what kind of device it is.
    function deviceName(name: var, kind: string): string {
        if (name)
            return name;
        switch (kind) {
        case "phone":
        case "foldable":
            return I18n.tr("Android phone");
        case "tablet":
            return I18n.tr("Tablet");
        case "laptop":
            return I18n.tr("Laptop");
        default:
            return I18n.tr("Nearby device");
        }
    }

    function deviceIcon(kind: string): string {
        switch (kind) {
        case "phone":
        case "foldable":
            return "smartphone";
        case "tablet":
            return "tablet";
        case "laptop":
            return "laptop";
        default:
            return "devices";
        }
    }

    // Icon of a file by its MIME type, else by its extension.
    function fileIcon(name: string, mime: string): string {
        const type = (mime ?? "").split("/")[0];
        const ext = name.includes(".") ? name.split(".").pop().toLowerCase() : "";
        if (type === "image" || ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "bmp", "svg", "tif", "tiff"].includes(ext))
            return "image";
        if (type === "video" || ["mp4", "mov", "mkv", "webm", "avi", "m4v", "3gp"].includes(ext))
            return "movie";
        if (type === "audio" || ["mp3", "m4a", "flac", "ogg", "opus", "wav", "aac"].includes(ext))
            return "music_note";
        if (["zip", "7z", "rar", "tar", "gz", "xz", "zst", "apk"].includes(ext) || /zip|compressed|archive|android\.package/.test(mime ?? ""))
            return "package";
        if (type === "text" || ["pdf", "txt", "md", "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp", "epub"].includes(ext) || /pdf|document|sheet|presentation/.test(mime ?? ""))
            return "description";
        return "draft";
    }

    // 5033165 -> "4.80 MB" (with a decimal comma in a German locale).
    function formatSize(bytes: real): string {
        return Fmt.bytes(bytes, Qt.locale());
    }

    function accept(id: int, dir: string): void {
        Persist.quickShareDir = dir;
        send({
            cmd: "accept",
            id: id,
            dir: dir
        });
    }

    function reject(id: int): void {
        send({
            cmd: "reject",
            id: id
        });
    }

    function cancel(id: int): void {
        send({
            cmd: "cancel",
            id: id
        });
    }

    // cb receives the whole reply ({ dir, parent, dirs, error }); on failure { error }.
    function listDirs(dir: string, cb: var): void {
        request({
            cmd: "dirs",
            dir: dir
        }, cb);
    }

    function makeDir(parent: string, name: string, cb: var): void {
        request({
            cmd: "mkdir",
            parent: parent,
            name: name
        }, cb);
    }

    // Renames this computer for phones: store it in the config, then have the
    // backend read it again.
    function setName(name: string): void {
        if (nameProc.running)
            return;
        nameProc.command = [Quickshell.env("VELA_BIN") || "vela", "set", "quickshare.name", "--", name];
        nameProc.running = true;
    }

    // "Sichtbar als …", or the progress of the transfer that is running.
    function subtitle(): string {
        if (!enabled)
            return I18n.tr("Off");
        for (const id in transfers) {
            const t = transfers[id];
            if (t.state !== "transferring")
                continue;
            const percent = t.total > 0 ? Math.floor(100 * t.done / t.total) : 0;
            return I18n.tr(t.direction === "in" ? "Receiving … %1 %" : "Sending … %1 %", percent);
        }
        return I18n.tr("Visible as “%1”", name);
    }

    // False when the backend is down; nothing was sent then.
    function send(cmd: var): bool {
        if (!proc.running)
            return false;
        proc.write(JSON.stringify(cmd) + "\n");
        return true;
    }

    function request(cmd: var, cb: var): void {
        const req = nextReq++;
        cmd.req = req;
        if (!send(cmd)) {
            cb({
                error: I18n.tr("Quick Share is not running")
            });
            return;
        }
        const p = Object.assign({}, pending);
        p[req] = cb;
        pending = p;
    }

    function settle(req: var, reply: var): void {
        const cb = pending[req];
        if (!cb)
            return;
        const p = Object.assign({}, pending);
        delete p[req];
        pending = p;
        cb(reply);
    }

    function mergePicked(reply: var): void {
        const have = new Set(pendingFiles.map(f => f.path));
        const added = (reply.files ?? []).filter(f => !have.has(f.path));
        if (added.length > 0)
            pendingFiles = pendingFiles.concat(added);
        if (reply.error)
            failed(reply.error);
    }

    function dropIncoming(id: int): void {
        if (incoming.some(o => o.id === id))
            incoming = incoming.filter(o => o.id !== id);
    }

    // Nothing of the old backend survives: its ids and requests are gone.
    function reset(): void {
        connected = false;
        devices = [];
        transfers = ({});
        sendQueue = [];
        sentTo = ({});
        incoming = [];
        const old = pending;
        pending = ({});
        for (const req in old)
            old[req]({
                error: I18n.tr("Quick Share stopped")
            });
    }

    function apply(line: string): void {
        let m;
        try {
            m = JSON.parse(line);
        } catch (e) {
            console.warn("quickshare: bad line", e);
            return;
        }
        switch (m.type) {
        case "hello":
            name = m.name;
            ble = !!m.ble;
            const first = !connected;
            connected = true;
            if (first && discovering)
                send({
                    cmd: "discover",
                    on: true
                });
            break;
        case "devices":
            devices = m.devices;
            break;
        case "incoming":
            if (!incoming.some(o => o.id === m.id))
                incoming = incoming.concat([m]);
            break;
        case "transfer":
            // The first line of a new outgoing transfer answers the oldest send.
            if (m.direction === "out" && !(m.id in transfers) && sendQueue.length > 0) {
                sentTo = Object.assign({}, sentTo, {
                    [m.id]: sendQueue[0]
                });
                sendQueue = sendQueue.slice(1);
            }
            const t = Object.assign({}, transfers, {
                [m.id]: m
            });
            if (finalStates.includes(m.state)) {
                dropIncoming(m.id);
                // Keep the newest finished ones for the UI (ids only grow).
                const over = Object.keys(t).map(Number).filter(i => finalStates.includes(t[i].state)).sort((a, b) => a - b).slice(0, -maxFinished);
                if (over.length > 0) {
                    const s = Object.assign({}, sentTo);
                    for (const i of over) {
                        delete t[i];
                        delete s[i];
                    }
                    sentTo = s;
                }
            }
            transfers = t;
            break;
        case "text":
            if (m.kind === "text")
                Quickshell.clipboardText = m.text;
            break;
        case "picked":
        case "dirs":
        case "mkdir":
            if (m.req !== null && m.req !== undefined)
                settle(m.req, m);
            break;
        case "error":
            // A send that never started: no transfer line will answer it.
            if (typeof m.device === "string") {
                const i = sendQueue.indexOf(m.device);
                if (i >= 0)
                    sendQueue = sendQueue.slice(0, i).concat(sendQueue.slice(i + 1));
            }
            // An error to a request goes to its callback only.
            if (m.req !== null && m.req !== undefined)
                settle(m.req, {
                    error: m.message
                });
            else
                failed(m.message);
            break;
        }
    }

    onEnabledChanged: {
        if (!enabled) {
            discovering = false;
            retry.stop();
            backoff = false;
            reset();
        }
    }

    Process {
        id: proc

        command: [Quickshell.env("VELA_BIN") || "vela", "quickshare", "serve"]
        running: root.enabled && !root.backoff
        stdinEnabled: true
        stdout: SplitParser {
            onRead: line => root.apply(line)
        }
        stderr: SplitParser {
            onRead: line => console.warn("quickshare:", line)
        }
        onExited: code => {
            root.reset();
            if (!root.enabled)
                return;
            console.warn("quickshare: backend exited with", code);
            root.backoff = true;
            retry.start();
        }
    }

    Timer {
        id: retry

        interval: 1500
        onTriggered: root.backoff = false
    }

    Process {
        id: nameProc

        stderr: StdioCollector {
            id: nameErr
        }
        onExited: code => {
            if (code !== 0)
                root.failed(nameErr.text.trim() || I18n.tr("Could not save the name"));
            else if (proc.running)
                root.send({
                    cmd: "reload"
                });
        }
    }
}
