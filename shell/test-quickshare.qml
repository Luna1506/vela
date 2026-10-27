// Quick Share views with test data set straight on the QuickShare singleton
// (no backend runs): both tabs of the detail view, the helpers they use, and
// a German translation for every string they show.
//   qs -p test-quickshare.qml   (scripts/shell-test.sh); prints PASS or FAIL
import QtQuick
import Quickshell
import qs
import qs.components
import qs.details
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
        "Android phone", "Tablet", "Laptop", "Nearby device"
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

    function translations(): void {
        const missing = strings.filter(s => !(s in I18n.translations.de));
        check(missing.length === 0, "no German for " + JSON.stringify(missing));
    }

    Component.onCompleted: {
        step(testData);
        step(detailView);
        later.start();
    }

    Timer {
        id: later

        interval: 100
        onTriggered: {
            root.step(root.afterDestroy);
            root.step(root.translations);
            root.finish();
        }
    }
}
