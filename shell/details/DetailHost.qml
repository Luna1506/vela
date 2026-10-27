import QtQuick
import qs
import qs.components
import qs.services

// Floating card left of the panel that shows the active detail view.
// Only one detail is shown at a time; switching swaps the content.
Item {
    id: root

    property real maxHeight: 0
    // Keeps the last view loaded during the close animation.
    property string current: ""
    property real progress: ShellState.detail !== "" ? 1 : 0

    Behavior on progress {
        Anim {
            easing.bezierCurve: Theme.anim.emphasizedDecel
        }
    }

    readonly property Component component: {
        switch (current) {
        case "wifi":
            return wifiDetail;
        case "bluetooth":
            return bluetoothDetail;
        case "audio":
            return audioDetail;
        case "power":
            return powerDetail;
        case "quickshare":
            return quickShareDetail;
        default:
            return null;
        }
    }

    width: Theme.size.detailWidth
    height: Math.min(maxHeight, (loader.item?.implicitHeight ?? 0) + 2 * Theme.size.panelPadding)
    visible: progress > 0
    opacity: progress

    transform: Translate {
        x: (1 - root.progress) * Theme.anim.slideDistance
    }

    Behavior on height {
        Anim {}
    }

    Connections {
        target: ShellState

        function onDetailChanged(): void {
            if (ShellState.detail !== "")
                root.current = ShellState.detail;
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.colors.panel
        radius: Theme.radius.panel
        border.width: Theme.size.border
        border.color: Theme.colors.outline

        // Swallow clicks so they don't close the panel.
        MouseArea {
            anchors.fill: parent
        }

        Loader {
            id: loader

            anchors.fill: parent
            anchors.margins: Theme.size.panelPadding
            active: root.visible
            sourceComponent: root.component
        }
    }

    Component {
        id: wifiDetail

        WifiDetail {}
    }

    Component {
        id: bluetoothDetail

        BluetoothDetail {}
    }

    Component {
        id: audioDetail

        AudioDetail {}
    }

    Component {
        id: powerDetail

        PowerDetail {}
    }

    Component {
        id: quickShareDetail

        QuickShareDetail {}
    }
}
