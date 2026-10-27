import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Wayland
import qs
import qs.components
import qs.services

// Opens in the middle of the focused monitor whenever a phone offers
// something, whether the panel is open or not: who sends what, the PIN, the
// folder to save to, Reject (Esc) / Accept (Enter). After accepting, the same
// window shows the progress until the transfer is done, and an error if it
// fails. Offers that come meanwhile wait their turn.
PanelWindow {
    id: root

    // Offers accepted here, by id: { offer, dir, background, dismissed,
    // cancelled }. The offer is kept for its sender (transfer lines carry no
    // kind).
    property var accepted: ({})
    // Offers rejected here whose final line hasn't come yet.
    property var rejected: []

    function stateOf(id: int): string {
        return QuickShare.transfers[id]?.state ?? "waiting";
    }

    readonly property var waitingOffers: QuickShare.incoming.filter(o => !rejected.includes(o.id) && stateOf(o.id) === "waiting")

    // An accepted transfer that ended badly and hasn't been closed yet.
    readonly property int outcomeId: {
        for (const id in accepted) {
            const a = accepted[id];
            const state = stateOf(Number(id));
            if (!a.dismissed && (state === "failed" || state === "no-space" || state === "cancelled" && !a.cancelled))
                return Number(id);
        }
        return -1;
    }
    // The newest accepted transfer that is running in front.
    readonly property int runningId: {
        let found = -1;
        for (const id in accepted) {
            const a = accepted[id];
            if (stateOf(Number(id)) === "transferring" && !a.background && !a.cancelled)
                found = Math.max(found, Number(id));
        }
        return found;
    }

    // "offer", "progress", "result" or "" (nothing to show). A transfer in
    // front only runs after its offer was accepted here, so it never hides
    // an undecided one; an outcome waits until no offer and no transfer in
    // front is left, so it never replaces what is being decided.
    readonly property string liveMode: runningId >= 0 ? "progress" : waitingOffers.length > 0 ? "offer" : outcomeId >= 0 ? "result" : ""
    readonly property bool shown: liveMode !== ""
    readonly property var liveOffer: liveMode === "offer" ? waitingOffers[0] : null
    readonly property int liveTransferId: liveMode === "result" ? outcomeId : liveMode === "progress" ? runningId : -1
    readonly property var liveTransfer: QuickShare.transfers[transferId] ?? null

    // What is on screen; kept while the window fades out.
    property string mode: ""
    property var offer: null
    property int transferId: -1
    property var transfer: null
    property var transferOffer: null

    onLiveModeChanged: {
        if (liveMode !== "")
            mode = liveMode;
    }
    onLiveOfferChanged: {
        if (!liveOffer)
            return;
        // A new offer: the browser opens in the last folder again.
        const fresh = offer?.id !== liveOffer.id;
        offer = liveOffer;
        if (!fresh)
            return;
        disarm();
        if (browser)
            browser.open(QuickShare.lastDir);
    }
    onLiveTransferIdChanged: {
        if (liveTransferId < 0)
            return;
        transferId = liveTransferId;
        transferOffer = accepted[liveTransferId]?.offer ?? null;
    }
    onLiveTransferChanged: {
        if (!liveTransfer)
            return;
        transfer = liveTransfer;
        measure(liveTransfer);
    }
    onShownChanged: {
        if (shown)
            open();
    }
    Component.onCompleted: {
        if (shown)
            open();
    }

    // A text or link (and no files): { kind, title }, else null.
    function textOf(o: var): var {
        return o && o.files.length === 0 ? o.text ?? null : null;
    }

    readonly property bool isText: textOf(offer) !== null
    readonly property var browser: browserLoader.item
    // Why accepting failed. The backend's usual answer is that the folder
    // is gone; anything else is shown as it comes.
    readonly property string offerError: {
        const message = offer ? QuickShare.offerErrors[offer.id] ?? "" : "";
        if (message === "")
            return "";
        return message.endsWith("is not a folder") ? I18n.tr("This folder doesn't exist (any more) – choose another one.") : I18n.tr("Cannot save here: %1", message);
    }
    // The folder may be gone: list it again, which goes up if it is.
    onOfferErrorChanged: {
        if (offerError !== "" && browser)
            browser.load(browser.dir);
    }
    // Accepted, the backend hasn't answered yet.
    readonly property bool busy: offer !== null && offer.id in accepted && !(offer.id in QuickShare.offerErrors) && stateOf(offer.id) === "waiting"
    readonly property string sender: offer ? QuickShare.deviceName(offer.device, offer.kind) : ""
    readonly property string pin: offer?.pin ?? ""

    readonly property string title: {
        if (!offer)
            return "";
        const text = textOf(offer);
        if (text)
            return I18n.tr(text.kind === "url" ? "%1 wants to send a link" : "%1 wants to send a text", sender);
        return offer.files.length === 1 ? I18n.tr("%1 wants to send a file", sender) : I18n.tr("%1 wants to send %2 files", sender, offer.files.length);
    }
    // What kind of files and how much, or the text's title.
    readonly property string subtitle: {
        if (!offer)
            return "";
        const text = textOf(offer);
        if (text)
            return text.title ?? "";
        const size = QuickShare.formatSize(offer.total);
        if (offer.files.length === 1)
            return offer.files[0].name + " · " + size;
        const counts = {
            image: 0,
            movie: 0,
            music_note: 0,
            other: 0
        };
        for (const f of offer.files) {
            const icon = QuickShare.fileIcon(f.name, f.mime);
            counts[icon in counts ? icon : "other"]++;
        }
        const words = [["image", "1 photo", "%1 photos"], ["movie", "1 video", "%1 videos"], ["music_note", "1 audio file", "%1 audio files"], ["other", "1 file", "%1 files"]];
        const parts = words.filter(w => counts[w[0]] > 0).map(w => counts[w[0]] === 1 ? I18n.tr(w[1]) : I18n.tr(w[2], counts[w[0]]));
        return parts.join(", ") + " · " + size;
    }

    readonly property string progressTitle: I18n.tr("Receiving from %1", QuickShare.deviceName(transferOffer?.device, transferOffer?.kind ?? ""))
    readonly property real progress: transfer && transfer.total > 0 ? Math.min(1, transfer.done / transfer.total) : 0
    readonly property string progressText: {
        if (!transfer)
            return "";
        const amount = [QuickShare.formatSize(transfer.done), QuickShare.formatSize(transfer.total)];
        if (rate <= 0 || transfer.done >= transfer.total)
            return I18n.tr("%1 of %2", ...amount);
        const left = (transfer.total - transfer.done) / rate;
        const eta = left < 60 ? I18n.tr("%1 s", Math.max(1, Math.round(left))) : I18n.duration(left);
        return I18n.tr("%1 of %2 · about %3 left", ...amount, eta);
    }
    readonly property string resultTitle: I18n.tr("Not received")
    readonly property string resultText: {
        switch (transfer?.state) {
        case "no-space":
            return I18n.tr("Not enough free space in this folder.");
        case "failed":
            return transfer.error ? I18n.tr("The transfer failed: %1", transfer.error) : I18n.tr("The transfer failed.");
        case "cancelled":
            // The phone, or the Wi-Fi in between.
            return I18n.tr("The transfer was cancelled.");
        default:
            return "";
        }
    }

    // Bytes per second since the transfer's first progress line.
    property var sample: null
    property real rate: 0

    function measure(t: var): void {
        if (t.state !== "transferring")
            return;
        const now = Date.now();
        if (!sample || sample.id !== t.id) {
            sample = {
                id: t.id,
                time: now,
                done: t.done
            };
            rate = 0;
        } else if (now - sample.time >= 1000 && t.done > sample.done) {
            rate = (t.done - sample.done) * 1000 / (now - sample.time);
        }
    }

    property ShellScreen hostScreen: null

    // Keys and the decision buttons work only a moment after the window
    // opens or shows a new offer: an Enter or Esc typed for another window
    // must not accept or reject what just popped up.
    property bool armed: false

    function disarm(): void {
        armed = false;
        armTimer.restart();
    }

    function open(): void {
        hostScreen = ShellState.focusedScreen();
        disarm();
        scope.forceActiveFocus();
    }

    function mark(id: int, change: var): void {
        if (!(id in accepted))
            return;
        accepted = Object.assign({}, accepted, {
            [id]: Object.assign({}, accepted[id], change)
        });
        prune();
    }

    // Forgets transfers that are over and need nothing more from the window.
    function prune(): void {
        const keep = {};
        let dropped = false;
        for (const id in accepted) {
            const a = accepted[id];
            const t = QuickShare.transfers[id];
            const waiting = QuickShare.incoming.some(o => o.id === Number(id));
            const over = t ? QuickShare.finalStates.includes(t.state) && (t.state === "done" || t.state === "rejected" || a.dismissed || a.cancelled) : !waiting;
            if (over)
                dropped = true;
            else
                keep[id] = a;
        }
        if (dropped)
            accepted = keep;
        const stillThere = rejected.filter(id => QuickShare.incoming.some(o => o.id === id));
        if (stillThere.length !== rejected.length)
            rejected = stillThere;
    }

    function accept(): void {
        if (liveMode !== "offer" || busy || !armed)
            return;
        const dir = isText ? "" : browser?.dir ?? "";
        accepted = Object.assign({}, accepted, {
            [offer.id]: {
                offer: offer,
                dir: dir,
                background: false,
                dismissed: false,
                cancelled: false
            }
        });
        QuickShare.accept(offer.id, dir);
    }

    function reject(): void {
        if (liveMode !== "offer" || !armed)
            return;
        QuickShare.reject(offer.id);
        rejected = rejected.concat([offer.id]);
    }

    // "In the background": only this window goes, the transfer goes on.
    function toBackground(): void {
        mark(transferId, {
            background: true
        });
    }

    function cancelTransfer(): void {
        QuickShare.cancel(transferId);
        mark(transferId, {
            cancelled: true
        });
    }

    function dismiss(): void {
        mark(transferId, {
            dismissed: true
        });
    }

    function handleKey(event: var): void {
        // Held keys and keys right after opening are swallowed.
        if (event.isAutoRepeat || !armed) {
            event.accepted = shown;
            return;
        }
        const enter = event.key === Qt.Key_Return || event.key === Qt.Key_Enter;
        const esc = event.key === Qt.Key_Escape;
        if (liveMode === "offer" && enter)
            accept();
        else if (liveMode === "offer" && esc)
            reject();
        else if (liveMode === "progress" && esc)
            toBackground();
        else if (liveMode === "result" && (esc || enter))
            dismiss();
        else
            return;
        event.accepted = true;
    }

    Timer {
        id: armTimer

        interval: Theme.anim.armDelay
        onTriggered: root.armed = true
    }

    Connections {
        target: QuickShare

        function onTransfersChanged(): void {
            root.prune();
        }

        function onIncomingChanged(): void {
            root.prune();
        }
    }

    visible: hostScreen !== null && (shown || card.opacity > 0)
    screen: hostScreen
    color: "transparent"
    implicitWidth: Theme.size.quickShareWindowWidth
    implicitHeight: card.implicitHeight
    exclusionMode: ExclusionMode.Ignore
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.namespace: "vela-quickshare"
    // Enter and Esc decide an offer; a running transfer doesn't hold on to
    // the keyboard.
    WlrLayershell.keyboardFocus: !shown ? WlrKeyboardFocus.None : mode === "offer" ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.OnDemand

    Rectangle {
        id: card

        width: parent.width
        implicitHeight: content.implicitHeight + 2 * Theme.spacing.lg
        height: implicitHeight
        radius: Theme.radius.panel
        color: Theme.colors.panel
        border.width: Theme.size.border
        border.color: Theme.colors.outline
        opacity: root.shown ? 1 : 0
        scale: root.shown ? 1 : Theme.anim.popInScale

        Behavior on opacity {
            Anim {}
        }

        Behavior on scale {
            Anim {}
        }

        FocusScope {
            id: scope

            anchors.fill: parent
            focus: true
            Keys.onPressed: event => root.handleKey(event)

            ColumnLayout {
                id: content

                anchors.fill: parent
                anchors.margins: Theme.spacing.lg
                spacing: Theme.spacing.md

                // Who sends what.
                RowLayout {
                    Layout.fillWidth: true
                    visible: root.mode === "offer"
                    spacing: Theme.spacing.md

                    Chip {
                        icon: QuickShare.deviceIcon(root.offer?.kind ?? "")
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        StyledText {
                            Layout.fillWidth: true
                            text: root.title
                            font.pixelSize: Theme.font.title
                            font.weight: Theme.font.weightMedium
                            wrapMode: Text.WordWrap
                        }

                        StyledText {
                            Layout.fillWidth: true
                            text: root.subtitle
                            color: Theme.colors.textMuted
                            font.pixelSize: Theme.font.small
                        }
                    }
                }

                // The PIN, to compare with the phone.
                Rectangle {
                    Layout.fillWidth: true
                    visible: root.mode === "offer" && root.pin !== ""
                    implicitHeight: pinRow.implicitHeight + 2 * Theme.spacing.sm
                    radius: Theme.radius.card
                    color: Theme.colors.chip

                    RowLayout {
                        id: pinRow

                        anchors.fill: parent
                        anchors.leftMargin: Theme.spacing.md
                        anchors.rightMargin: Theme.spacing.md
                        spacing: Theme.spacing.md

                        StyledText {
                            Layout.fillWidth: true
                            text: I18n.tr("Code (must match the phone)")
                            color: Theme.colors.textMuted
                            font.pixelSize: Theme.font.small
                        }

                        StyledText {
                            text: root.pin
                            color: Theme.colors.primary
                            font.pixelSize: Theme.font.large
                            font.weight: Theme.font.weightSemiBold
                            font.letterSpacing: Theme.font.large * 0.3
                        }
                    }
                }

                SectionHeader {
                    visible: browserLoader.active
                    text: I18n.tr("Save to")
                }

                Loader {
                    id: browserLoader

                    Layout.fillWidth: true
                    // An inactive Loader keeps the height of its last item.
                    visible: active
                    active: root.mode === "offer" && root.offer !== null && !root.isText

                    sourceComponent: FolderBrowser {
                        startDir: QuickShare.lastDir
                        onFieldClosed: scope.forceActiveFocus()
                    }
                }

                StyledText {
                    Layout.fillWidth: true
                    visible: root.mode === "offer" && root.offerError !== ""
                    text: root.offerError
                    color: Theme.colors.error
                    font.pixelSize: Theme.font.small
                    wrapMode: Text.WordWrap
                }

                RowLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: Theme.spacing.xs
                    visible: root.mode === "offer"
                    spacing: Theme.spacing.sm

                    StyledText {
                        Layout.fillWidth: true
                        text: I18n.tr("Enter accepts · Esc rejects")
                        color: Theme.colors.textDisabled
                        font.pixelSize: Theme.font.small
                    }

                    PillButton {
                        text: I18n.tr("Reject")
                        enabled: root.armed
                        onClicked: root.reject()
                    }

                    PillButton {
                        style: "filled"
                        text: I18n.tr("Accept")
                        enabled: root.armed && !root.busy
                        onClicked: root.accept()
                    }
                }

                // Receiving, or how it ended.
                RowLayout {
                    Layout.fillWidth: true
                    visible: root.mode !== "offer"
                    spacing: Theme.spacing.md

                    Chip {
                        icon: root.mode === "result" ? "error" : "download"
                        iconColor: root.mode === "result" ? Theme.colors.error : Theme.colors.primary
                        color: root.mode === "result" ? Theme.colors.errorSurface : Theme.colors.accentChip
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        StyledText {
                            Layout.fillWidth: true
                            text: root.mode === "result" ? root.resultTitle : root.progressTitle
                            font.weight: Theme.font.weightMedium
                        }

                        StyledText {
                            Layout.fillWidth: true
                            // Who it was from, or how far it got.
                            text: root.mode === "result" ? QuickShare.deviceName(root.transferOffer?.device, root.transferOffer?.kind ?? "") : root.progressText
                            color: Theme.colors.textMuted
                            font.pixelSize: Theme.font.small
                        }
                    }

                    IconButton {
                        visible: root.mode === "progress"
                        icon: "close"
                        iconColor: Theme.colors.textMuted
                        onClicked: root.cancelTransfer()
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    visible: root.mode === "progress"
                    implicitHeight: Theme.size.progressBarHeight
                    radius: height / 2
                    color: Theme.colors.surfaceHighest

                    Rectangle {
                        height: parent.height
                        width: Math.max(height, parent.width * root.progress)
                        radius: height / 2
                        color: Theme.colors.primary

                        Behavior on width {
                            Anim {}
                        }
                    }
                }

                StyledText {
                    Layout.fillWidth: true
                    visible: root.mode === "result"
                    text: root.resultText
                    color: Theme.colors.error
                    wrapMode: Text.WordWrap
                }

                RowLayout {
                    Layout.fillWidth: true
                    visible: root.mode !== "offer"
                    spacing: Theme.spacing.sm

                    Item {
                        Layout.fillWidth: true
                    }

                    PillButton {
                        visible: root.mode === "progress"
                        text: I18n.tr("In the background")
                        onClicked: root.toBackground()
                    }

                    PillButton {
                        visible: root.mode === "result"
                        text: I18n.tr("Close")
                        onClicked: root.dismiss()
                    }
                }
            }
        }
    }

    // Round icon badge at the start of a header.
    component Chip: Rectangle {
        property string icon
        property color iconColor: Theme.colors.primary

        implicitWidth: Theme.size.tileIcon + Theme.spacing.sm
        implicitHeight: implicitWidth
        radius: height / 2
        color: Theme.colors.accentChip

        MaterialIcon {
            anchors.centerIn: parent
            icon: parent.icon
            size: Theme.icon.large
            color: parent.iconColor
        }
    }
}
