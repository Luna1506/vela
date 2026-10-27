import QtQuick
import QtQuick.Layouts
import qs
import qs.components
import qs.services

// Quick Share: send files to devices nearby, or be visible to them.
ColumnLayout {
    id: root

    property bool usable: QuickShare.enabled
    readonly property alias page: tab.item
    // Devices are looked for only while the send tab is on screen.
    readonly property bool wantsDevices: usable && ShellState.panelOpen && ShellState.detail === "quickshare" && ShellState.quickShareTab === "send"

    onWantsDevicesChanged: QuickShare.discover(wantsDevices)
    Component.onCompleted: {
        if (wantsDevices)
            QuickShare.discover(true);
    }
    Component.onDestruction: {
        if (QuickShare.discovering)
            QuickShare.discover(false);
    }

    spacing: Theme.spacing.md

    DetailHeader {
        Layout.fillWidth: true
        title: I18n.tr("Quick Share")
        showSwitch: true
        checked: QuickShare.enabled
        onToggled: QuickShare.toggle()
    }

    SegmentedButtons {
        Layout.fillWidth: true
        visible: root.usable
        model: [
            {
                key: "send",
                icon: "upload",
                text: I18n.tr("Send")
            },
            {
                key: "receive",
                icon: "download",
                text: I18n.tr("Receive")
            }
        ]
        current: ShellState.quickShareTab
        onSelected: key => ShellState.quickShareTab = key
    }

    EmptyState {
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacing.xl
        Layout.bottomMargin: Theme.spacing.xl
        visible: !root.usable
        icon: "share"
        title: I18n.tr("Quick Share is off")
        subtitle: I18n.tr("Turn it on to swap files with phones nearby.")
    }

    Loader {
        id: tab

        Layout.fillWidth: true
        Layout.fillHeight: true
        active: root.usable
        sourceComponent: ShellState.quickShareTab === "send" ? sendTab : receiveTab
    }

    Component {
        id: sendTab

        QuickShareSend {}
    }

    Component {
        id: receiveTab

        QuickShareReceive {}
    }
}
