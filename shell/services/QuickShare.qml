pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io
import qs

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

    // Something went wrong the user should hear about.
    signal failed(string message)

    // A transfer in one of these states is over.
    readonly property var finalStates: ["done", "rejected", "cancelled", "failed", "no-space"]

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
        send({
            cmd: "send",
            device: deviceId,
            files: pendingFiles.map(f => f.path)
        });
        pendingFiles = [];
    }

    function accept(id: int, dir: string): void {
        Persist.quickShareDir = dir;
        send({
            cmd: "accept",
            id: id,
            dir: dir
        });
        dropIncoming(id);
    }

    function reject(id: int): void {
        send({
            cmd: "reject",
            id: id
        });
        dropIncoming(id);
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
        nameProc.command = [Quickshell.env("VELA_BIN") || "vela", "set", "quickshare.name", name];
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

    function send(cmd: var): bool {
        if (!proc.running) {
            failed(I18n.tr("Quick Share is not running"));
            return false;
        }
        proc.write(JSON.stringify(cmd) + "\n");
        return true;
    }

    function request(cmd: var, cb: var): void {
        const req = nextReq++;
        cmd.req = req;
        if (!send(cmd))
            return;
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
            transfers = Object.assign({}, transfers, {
                [m.id]: m
            });
            if (finalStates.includes(m.state))
                dropIncoming(m.id);
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
            if (m.req !== null && m.req !== undefined)
                settle(m.req, {
                    error: m.message
                });
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
