// Quick Share views with test data set straight on the QuickShare singleton
// (no backend runs): both tabs of the detail view, the window for incoming
// offers with its folder browser, the helpers they use, and a German
// translation for every string they show.
//   qs -p test-quickshare.qml   (scripts/shell-test.sh); prints PASS or FAIL
import QtQuick
import Quickshell
import qs
import qs.components
import qs.details
import qs.quickshare
import qs.services

ShellRoot {
    id: root

    // Every source string of the Quick Share views.
    readonly property var strings: [
        // Detail view and tabs
        "Quick Share", "Send", "Receive", "Quick Share is off", "Turn it on to swap files with phones nearby.",
        // Send
        "Files", "Choose files …", "Nearby devices", "Searching for devices…", "Ready", "Waiting for approval",
        "Waiting for approval · PIN %1", "Sending … %1 %", "Finished", "Rejected", "Cancelled", "Error",
        "On the phone, Quick Share must be set to “Everyone”.", "Bluetooth is off – phones may not be found.",
        // Receive
        "Visible to nearby devices", "On the phone, choose Quick Share and then “%1”", "Name", "Leave empty for the system name.",
        "Phone and PC must be on the same Wi-Fi.", "Phone doesn't find this PC? Turn off “Share with Apple devices” in Quick Share on the phone.",
        // Devices without a name
        "Android phone", "Tablet", "Laptop", "Nearby device",
        // Incoming offer
        "%1 wants to send a file", "%1 wants to send %2 files", "%1 wants to send a link", "%1 wants to send a text",
        "1 photo", "%1 photos", "1 video", "%1 videos", "1 audio file", "%1 audio files", "1 file", "%1 files",
        "Code (must match the phone)", "Save to", "Reject", "Accept", "Enter accepts · Esc rejects", "Cannot save here: %1",
        "This folder doesn't exist (any more) – choose another one.",
        // Folder browser
        "New folder", "Folder name", "Already exists", "Invalid name", "No subfolders", "Saving to %1",
        "“%1” can't be opened – showing the folder above.",
        // Progress and outcome
        "Receiving from %1", "%1 of %2", "%1 of %2 · about %3 left", "%1 s", "%1 min", "%1 h %2 min", "In the background", "Close",
        "Not received", "Not enough free space in this folder.", "The transfer failed.", "The transfer failed: %1", "The transfer was cancelled."
    ]

    property var failures: []

    function check(ok: bool, what: string): void {
        if (!ok)
            failures.push(what);
    }

    // Runs one step of the test; an exception counts as a failure.
    function step(fn: var): void {
        try {
            fn();
        } catch (e) {
            failures.push(String(e));
        }
    }

    function finish(): void {
        console.log(failures.length === 0 ? "PASS" : "FAIL " + JSON.stringify(failures));
        Qt.callLater(Qt.quit);
    }

    function create(name: string, props: var): var {
        const c = Qt.createComponent(name + ".qml");
        const o = c.status === Component.Ready ? c.createObject(host, props) : null;
        if (!o)
            failures.push(name + ": " + c.errorString());
        return o;
    }

    function transferLine(id: int, state: string, pin: string): string {
        return JSON.stringify({
            type: "transfer",
            id: id,
            direction: "out",
            device: "",
            state: state,
            done: 0,
            total: 7340032,
            pin: pin,
            error: "",
            dir: "",
            saved: []
        });
    }

    readonly property string home: Quickshell.env("HOME")

    function incomingLine(id: int, device: string, kind: string, files: var, text: var): string {
        return JSON.stringify({
            type: "incoming",
            id: id,
            device: device,
            kind: kind,
            pin: "4821",
            files: files,
            text: text,
            total: files.reduce((sum, f) => sum + f.size, 0)
        });
    }

    function inLine(id: int, state: string, done: real, error: string): string {
        return JSON.stringify({
            type: "transfer",
            id: id,
            direction: "in",
            device: "",
            state: state,
            done: done,
            total: 192937984,
            pin: "4821",
            error: error,
            dir: "",
            saved: []
        });
    }

    // A phone offers files (or text): the backend's two lines for it.
    function offer(id: int, device: string, kind: string, files: var, text: var): void {
        QuickShare.apply(incomingLine(id, device, kind, files, text));
        QuickShare.apply(inLine(id, "waiting", 0, ""));
    }

    function key(target: var, k: int, repeat: bool): bool {
        const event = {
            key: k,
            modifiers: Qt.NoModifier,
            isAutoRepeat: repeat === true,
            accepted: false
        };
        target.handleKey(event);
        return event.accepted;
    }

    Item {
        id: host

        width: 380
        height: 800
    }

    property var detail: null

    // Devices, files and a running send, and the helpers that show them.
    function testData(): void {
        QuickShare.devices = [
            {
                id: "PHN1",
                name: "Galaxy S24",
                kind: "phone"
            },
            {
                id: "TAB2",
                name: null,
                kind: "tablet"
            }
        ];
        QuickShare.pendingFiles = [
            {
                path: "/home/test/IMG_2041.jpg",
                name: "IMG_2041.jpg",
                size: 5033165
            },
            {
                path: "/home/test/Skript_Kapitel3.pdf",
                name: "Skript_Kapitel3.pdf",
                size: 2202010
            }
        ];
        // A send to the phone: the backend's first line for it carries no
        // device id; the singleton pairs it with the device it sent to.
        QuickShare.sendQueue = ["PHN1"];
        QuickShare.apply(transferLine(3, "waiting", "4821"));
        check(QuickShare.deviceTransfer("PHN1")?.id === 3, "the transfer belongs to the phone");
        check(QuickShare.deviceTransfer("TAB2") === null, "no transfer for the tablet");
        check(QuickShare.sendQueue.length === 0, "the send queue is used up");
        QuickShare.apply(transferLine(4, "waiting", ""));
        check(QuickShare.deviceTransfer("PHN1")?.id === 3, "a transfer nobody here started is not the phone's");
        QuickShare.sendQueue = ["TAB2"];
        QuickShare.apply(JSON.stringify({
            type: "error",
            message: "no device TAB2 nearby",
            device: "TAB2"
        }));
        check(QuickShare.sendQueue.length === 0, "a send that failed leaves the queue");

        check(QuickShare.deviceName("Galaxy S24", "phone") === "Galaxy S24", "named device");
        check(QuickShare.deviceName(null, "phone") === I18n.tr("Android phone"), "nameless phone");
        check(QuickShare.deviceName("", "tablet") === I18n.tr("Tablet"), "nameless tablet");
        check(QuickShare.deviceName(null, "laptop") === I18n.tr("Laptop"), "nameless laptop");
        check(QuickShare.deviceName(null, "car") === I18n.tr("Nearby device"), "nameless other device");
        check(JSON.stringify(["phone", "tablet", "laptop", "unknown"].map(QuickShare.deviceIcon)) === JSON.stringify(["smartphone", "tablet", "laptop", "devices"]), "device icons");
        check(JSON.stringify(["a.JPG", "b.pdf", "c.mp4", "d.mp3", "e.zip", "f.bin"].map(n => QuickShare.fileIcon(n, ""))) === JSON.stringify(["image", "description", "movie", "music_note", "package", "draft"]), "file icons by name");
        check(QuickShare.fileIcon("photo", "image/heic") === "image", "file icon by MIME type");
        check(/^4[.,]80 MB$/.test(QuickShare.formatSize(5033165)), "readable size: " + QuickShare.formatSize(5033165));

    }

    // The detail view is open: "send" looks for devices, "receive" not.
    function detailView(): void {
        ShellState.panelOpen = true;
        ShellState.detail = "quickshare";
        check(ShellState.quickShareTab === "receive", "the receive tab comes first");
        detail = create("details/QuickShareDetail", {
            usable: true
        });
        for (const tab of ["send", "receive", "send"]) {
            ShellState.quickShareTab = tab;
            check(detail?.page?.objectName === tab, tab + " tab is shown");
            check(QuickShare.discovering === (tab === "send"), tab + " tab: looking for devices " + QuickShare.discovering);
        }
        ShellState.closeDetail();
        check(!QuickShare.discovering, "closing the view stops looking for devices");
        ShellState.detail = "quickshare";
        check(QuickShare.discovering, "opening it again looks again");
        detail?.destroy();
    }

    // After the view is gone (destroy() takes effect later).
    function afterDestroy(): void {
        check(!QuickShare.discovering, "a destroyed view stops looking for devices");
        const off = create("details/QuickShareDetail", {
            usable: false
        });
        check(off !== null && off.page === null, "Quick Share off shows no tab");
    }

    // The hint about "Share with Apple devices" is shown whatever
    // `bleReceive` from `hello` says; the property still follows `hello`.
    function receiveHint(): void {
        const page = create("details/QuickShareReceive", {});
        const hint = page ? findChild(page, "appleHint") : null;
        check(hint !== null, "the receive tab has the hint");
        const wasConnected = QuickShare.connected;
        for (const on of [true, false]) {
            QuickShare.apply(JSON.stringify({
                type: "hello",
                name: QuickShare.name,
                port: 47392,
                ble: false,
                bleReceive: on
            }));
            check(QuickShare.bleReceive === on, "hello sets bleReceive " + on);
            check(hint?.visible === true, "hint shown " + hint?.visible + " with bleReceive " + on);
        }
        // A backend that is gone leaves no Bluetooth state behind.
        QuickShare.apply(JSON.stringify({
            type: "hello",
            name: QuickShare.name,
            port: 47392,
            ble: true,
            bleReceive: true
        }));
        QuickShare.reset();
        check(!QuickShare.ble && !QuickShare.bleReceive, "reset clears ble " + QuickShare.ble + " and bleReceive " + QuickShare.bleReceive);
        QuickShare.connected = wasConnected;
        page?.destroy();
    }

    function findChild(item: var, name: string): var {
        if (item.objectName === name)
            return item;
        for (const c of item.children) {
            const found = findChild(c, name);
            if (found)
                return found;
        }
        return null;
    }

    // The folder browser: starts in the last folder, walks the tree, goes up
    // from a folder that is gone and makes new folders.
    function folderBrowser(): void {
        const korea = home + "/Bilder/Korea";
        const b = create("quickshare/FolderBrowser", {
            startDir: korea
        });
        if (!b)
            return;
        check(b.dir === korea, "the browser starts in the last folder: " + b.dir);
        b.listed({
            dir: korea,
            parent: home + "/Bilder",
            dirs: ["Busan", "Campus", "Essen"],
            error: ""
        });
        check(JSON.stringify(b.dirs) === JSON.stringify(["Busan", "Campus", "Essen"]), "subfolders listed");
        check(JSON.stringify(b.crumbs.map(c => [c.home, c.label, c.path])) === JSON.stringify([[true, "", home], [false, "Bilder", home + "/Bilder"], [false, "Korea", korea]]), "path bar from home: " + JSON.stringify(b.crumbs));
        check(b.canGoUp, "up is possible below /");
        check(b.saveText === I18n.tr("Saving to %1", "~/Bilder/Korea"), "where it saves: " + b.saveText);

        // Into a subfolder, then up again.
        b.open(korea + "/Busan");
        check(b.dir === korea + "/Busan", "a click on a folder goes into it");
        b.goUp();
        check(b.dir === korea, "up goes to the parent");

        // A folder that is gone: one level up, with a hint.
        b.listed({
            dir: korea + "/Weg",
            parent: korea,
            dirs: [],
            error: korea + "/Weg: No such file or directory (os error 2)"
        });
        check(b.dir === korea, "a folder that is gone goes up: " + b.dir);
        check(b.hint === I18n.tr("“%1” can't be opened – showing the folder above.", "Weg"), "hint about the folder that is gone: " + b.hint);
        b.listed({
            dir: korea,
            parent: home + "/Bilder",
            dirs: ["Busan"],
            error: ""
        });
        check(b.hint !== "", "the hint stays after going up");
        b.open(korea + "/Busan");
        check(b.hint === "", "going somewhere else clears the hint");
        b.listed({
            dir: korea,
            parent: home + "/Bilder",
            dirs: ["Busan"],
            error: ""
        });

        // The root: no way up, the path bar starts at "/".
        b.listed({
            dir: "/",
            parent: "",
            dirs: ["etc", "home"],
            error: ""
        });
        check(!b.canGoUp, "no way up from /");
        check(JSON.stringify(b.crumbs.map(c => c.label)) === JSON.stringify(["/"]), "path bar of /");
        b.listed({
            dir: korea,
            parent: home + "/Bilder",
            dirs: ["Busan"],
            error: ""
        });

        // New folder: Esc closes the field, bad and existing names are refused.
        b.startNewFolder();
        check(b.creating, "New folder opens the field");
        check(key(b, Qt.Key_Escape) && !b.creating, "Esc in the field only closes the field");
        b.startNewFolder();
        b.folderName = "Busan";
        b.createFolder();
        check(b.creating && b.fieldError === I18n.tr("Already exists"), "an existing name: " + b.fieldError);
        b.folderName = "a/b";
        b.createFolder();
        check(b.fieldError === I18n.tr("Invalid name"), "a bad name: " + b.fieldError);
        b.made({
            path: "",
            error: "File exists (os error 17)"
        });
        check(b.fieldError === I18n.tr("Already exists"), "the backend says it exists: " + b.fieldError);
        b.made({
            path: "",
            error: "not a valid folder name"
        });
        check(b.fieldError === I18n.tr("Invalid name"), "the backend refuses the name: " + b.fieldError);
        b.folderName = "Seoul Oktober";
        check(key(b, Qt.Key_Return), "Enter in the field is taken by the field");
        b.made({
            path: korea + "/Seoul Oktober",
            error: ""
        });
        check(!b.creating && b.dir === korea + "/Seoul Oktober", "a new folder is entered: " + b.dir);
        b.destroy();
    }

    property var win: null

    // The window for incoming offers, fed with backend lines.
    function incomingWindow(): void {
        const korea = home + "/Bilder/Korea";
        // Accepting a text (dir "") must not forget the last folder.
        QuickShare.accept(99, "");
        check(QuickShare.lastDir === korea, "accepting without a folder keeps the last one: " + QuickShare.lastDir);
        win = create("quickshare/IncomingWindow", {});
        if (!win)
            return;
        check(!win.shown, "no offer, no window");

        const photos = [
            {
                name: "IMG_1.jpg",
                size: 4000000,
                mime: "image/jpeg"
            },
            {
                name: "IMG_2.jpg",
                size: 4000000,
                mime: "image/jpeg"
            },
            {
                name: "clip.mp4",
                size: 184000000,
                mime: "video/mp4"
            }
        ];
        offer(11, "Galaxy S24", "phone", photos, null);
        check(win.shown && win.mode === "offer", "an offer opens the window: " + win.mode);
        check(win.title === I18n.tr("%1 wants to send %2 files", "Galaxy S24", 3), "title: " + win.title);
        check(win.subtitle.startsWith(I18n.tr("%1 photos", 2) + ", " + I18n.tr("1 video") + " · "), "kinds and size: " + win.subtitle);
        check(win.pin === "4821", "the PIN is shown");
        check(win.browser !== null && win.browser.dir === korea, "the folder browser opens in the last folder");
        win.browser?.listed({
            dir: korea,
            parent: home + "/Bilder",
            dirs: ["Busan"],
            error: ""
        });

        win.browser?.open(korea + "/Busan");

        // A key that was meant for another window decides nothing.
        check(!win.armed, "not armed right after the offer appears");
        check(key(win, Qt.Key_Return) && !(11 in win.accepted), "Enter right after the offer appears does nothing");
        check(key(win, Qt.Key_Escape) && win.offer?.id === 11 && !win.rejected.includes(11), "Esc right after the offer appears does nothing");
        win.accept();
        check(!(11 in win.accepted), "the Accept button waits for the delay too");
    }

    // Lets a new offer's keys and buttons work without waiting the delay
    // (the first offer above waits for real).
    function ready(): void {
        check(!win.armed, "a new offer waits before taking keys");
        win.armed = true;
    }

    // The same window, after the arming delay.
    function incomingWindowArmed(): void {
        if (!win)
            return;
        const korea = home + "/Bilder/Korea";
        const photos = [
            {
                name: "IMG_1.jpg",
                size: 4000000,
                mime: "image/jpeg"
            },
            {
                name: "IMG_2.jpg",
                size: 4000000,
                mime: "image/jpeg"
            },
            {
                name: "clip.mp4",
                size: 184000000,
                mime: "video/mp4"
            }
        ];
        check(win.armed, "keys work after the delay");
        check(key(win, Qt.Key_Return, true) && !(11 in win.accepted), "an auto-repeated Enter does nothing");

        // Enter accepts into the open folder; the backend says it can't.
        check(key(win, Qt.Key_Return), "Enter is handled");
        check(win.accepted[11]?.dir === korea + "/Busan", "Enter accepts into the open folder: " + win.accepted[11]?.dir);
        check(win.mode === "offer" && win.busy, "waiting for the backend after accepting");
        QuickShare.apply(JSON.stringify({
            type: "error",
            id: 11,
            message: korea + "/Busan is not a folder"
        }));
        check(win.mode === "offer" && !win.busy, "a refused accept stays on the offer");
        check(win.offerError === I18n.tr("This folder doesn't exist (any more) – choose another one."), "the error is shown: " + win.offerError);
        QuickShare.apply(JSON.stringify({
            type: "error",
            id: 11,
            message: "disk on fire"
        }));
        check(win.offerError === I18n.tr("Cannot save here: %1", "disk on fire"), "another error as it comes: " + win.offerError);
        check(QuickShare.lastDir === korea, "a refused folder doesn't become the last one: " + QuickShare.lastDir);
        win.browser?.open(korea + "/Campus");
        win.accept();
        check(QuickShare.offerErrors[11] === undefined && win.offerError === "", "accepting again clears the error");

        // The backend starts receiving: progress in the same window.
        QuickShare.apply(JSON.stringify(Object.assign(JSON.parse(inLine(11, "transferring", 0, "")), {
            dir: korea + "/Campus"
        })));
        check(QuickShare.lastDir === korea + "/Campus", "receiving makes the folder the last one: " + QuickShare.lastDir);
        check(win.mode === "progress", "accepted: the progress is shown: " + win.mode);
        check(win.progressTitle === I18n.tr("Receiving from %1", "Galaxy S24"), "progress title: " + win.progressTitle);
        QuickShare.apply(inLine(11, "transferring", 96468992, ""));
        check(win.progress === 0.5, "progress: " + win.progress);
        check(win.progressText.startsWith(I18n.tr("%1 of %2", QuickShare.formatSize(96468992), QuickShare.formatSize(192937984))), "progress text: " + win.progressText);

        // In the background: the window goes; a new offer brings it back.
        win.toBackground();
        check(!win.shown, "In the background closes the window");
        offer(12, "", "phone", [], {
            kind: "url",
            title: "https://example.org"
        });
        check(win.shown && win.mode === "offer", "a new offer opens the window again");
        ready();
        check(win.title === I18n.tr("%1 wants to send a link", I18n.tr("Android phone")), "a link from a nameless phone: " + win.title);
        check(win.browser === null, "no folder browser for a link");
        offer(13, "Pixel", "phone", [], {
            kind: "text",
            title: "Hallo"
        });
        check(win.offer?.id === 12, "the oldest offer comes first");
        check(key(win, Qt.Key_Escape), "Esc is handled");
        check(win.offer?.id === 13, "after rejecting, the next offer in the queue");
        ready();
        check(win.title === I18n.tr("%1 wants to send a text", "Pixel"), "a text: " + win.title);
        win.accept();
        check(win.accepted[13]?.dir === "", "a text is accepted without a folder");
        QuickShare.apply(inLine(13, "transferring", 0, ""));
        QuickShare.apply(inLine(13, "done", 5, ""));
        QuickShare.apply(inLine(12, "rejected", 0, ""));
        check(!win.shown, "done: the window closes by itself");

        // The transfer in the background runs out of space: shown again.
        QuickShare.apply(inLine(11, "no-space", 0, ""));
        check(win.shown && win.mode === "result", "no space brings the window back: " + win.mode);
        check(win.resultTitle === I18n.tr("Not received"), "outcome title: " + win.resultTitle);
        check(win.resultText === I18n.tr("Not enough free space in this folder."), "no-space text: " + win.resultText);
        ready();
        check(key(win, Qt.Key_Escape) && !win.shown, "Close / Esc closes the outcome");

        // A nameless tablet: the progress keeps the name from the offer.
        offer(14, "", "tablet", [photos[0]], null);
        check(win.title === I18n.tr("%1 wants to send a file", I18n.tr("Tablet")), "one file from a tablet: " + win.title);
        check(win.subtitle.startsWith("IMG_1.jpg · "), "a single file shows its name: " + win.subtitle);
        ready();
        win.accept();
        QuickShare.apply(inLine(14, "transferring", 0, ""));
        check(win.progressTitle === I18n.tr("Receiving from %1", I18n.tr("Tablet")), "progress names the tablet: " + win.progressTitle);
        QuickShare.apply(inLine(14, "failed", 0, "connection reset"));
        check(win.shown && win.resultText === I18n.tr("The transfer failed: %1", "connection reset"), "failure text: " + win.resultText);
        win.dismiss();
        check(!win.shown, "Close closes the outcome");

        // Cancelling here closes the window without an outcome.
        offer(15, "Galaxy S24", "phone", photos, null);
        ready();
        win.accept();
        QuickShare.apply(inLine(15, "transferring", 0, ""));
        win.cancelTransfer();
        check(!win.shown, "cancelling closes the window");
        QuickShare.apply(inLine(15, "cancelled", 0, ""));
        check(!win.shown, "a transfer cancelled here shows no outcome");

        // A background transfer that fails waits until the offer on screen
        // is decided; the folder browser keeps its place meanwhile.
        offer(16, "Galaxy S24", "phone", photos, null);
        ready();
        win.accept();
        QuickShare.apply(inLine(16, "transferring", 0, ""));
        win.toBackground();
        offer(17, "Pixel", "phone", photos, null);
        win.browser?.open(korea + "/Busan");
        QuickShare.apply(inLine(16, "failed", 0, "connection reset"));
        check(win.mode === "offer" && win.offer?.id === 17 && win.browser?.dir === korea + "/Busan", "a failure in the background doesn't replace the offer: " + win.mode + " " + win.browser?.dir);
        ready();
        key(win, Qt.Key_Escape);
        check(win.mode === "result" && win.transferId === 16, "the failure shows once the offer is decided: " + win.mode);
        win.armed = true;
        win.dismiss();
        QuickShare.apply(inLine(17, "rejected", 0, ""));
        check(!win.shown, "nothing left to show");

        // Cancelled from the other side (or the Wi-Fi went): an outcome.
        offer(18, "Pixel", "phone", photos, null);
        ready();
        win.accept();
        QuickShare.apply(inLine(18, "transferring", 0, ""));
        QuickShare.apply(inLine(18, "cancelled", 0, ""));
        check(win.mode === "result" && win.resultText === I18n.tr("The transfer was cancelled."), "cancelled text: " + win.resultText);
        win.destroy();
    }

    function translations(): void {
        const missing = strings.filter(s => !(s in I18n.translations.de));
        check(missing.length === 0, "no German for " + JSON.stringify(missing));
    }

    Component.onCompleted: {
        // A second Persist write in one event loop turn reads back stale
        // until the next turn: the last folder is set here, the window is
        // tried in a later turn.
        Persist.quickShareDir = home + "/Bilder/Korea";
        step(testData);
        step(detailView);
        later.start();
    }

    Timer {
        id: later

        interval: 100
        onTriggered: {
            root.step(root.afterDestroy);
            root.step(root.receiveHint);
            root.step(root.folderBrowser);
            root.step(root.incomingWindow);
            armDelay.start();
        }
    }

    // Longer than the window's arming delay.
    Timer {
        id: armDelay

        interval: 1000
        onTriggered: {
            root.step(root.incomingWindowArmed);
            root.step(root.translations);
            root.finish();
        }
    }
}
