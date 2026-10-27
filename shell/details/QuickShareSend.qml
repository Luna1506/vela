import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Quickshell
import qs
import qs.components
import qs.services

// Send tab of the Quick Share view: the files to send (picked or dropped
// anywhere on the tab) and the devices nearby to send them to.
Item {
    id: root

    readonly property bool hasFiles: QuickShare.pendingFiles.length > 0

    function status(t: var): string {
        if (!t)
            return I18n.tr("Ready");
        switch (t.state) {
        case "waiting":
            return t.pin !== "" ? I18n.tr("Waiting for approval · PIN %1", t.pin) : I18n.tr("Waiting for approval");
        case "transferring":
            return I18n.tr("Sending … %1 %", t.total > 0 ? Math.floor(100 * t.done / t.total) : 0);
        case "done":
            return I18n.tr("Finished");
        case "rejected":
            return I18n.tr("Rejected");
        case "cancelled":
            return I18n.tr("Cancelled");
        default:
            return I18n.tr("Error");
        }
    }

    objectName: "send"
    implicitHeight: scroll.implicitHeight

    ScrollArea {
        id: scroll

        anchors.fill: parent

        ColumnLayout {
            width: parent.width
            spacing: Theme.spacing.sm

            SectionHeader {
                text: I18n.tr("Files")
            }

            Card {
                Layout.fillWidth: true
                visible: root.hasFiles
                implicitHeight: files.implicitHeight + 2 * Theme.spacing.xs

                Column {
                    id: files

                    anchors.fill: parent
                    anchors.margins: Theme.spacing.xs

                    Repeater {
                        model: ScriptModel {
                            values: QuickShare.pendingFiles
                        }

                        ListRow {
                            id: file

                            required property var modelData

                            width: parent.width
                            clickable: false
                            icon: QuickShare.fileIcon(modelData.name, "")
                            title: modelData.name
                            subtitle: QuickShare.formatSize(modelData.size)

                            IconButton {
                                anchors.verticalCenter: parent.verticalCenter
                                implicitWidth: Theme.size.pillButton
                                implicitHeight: Theme.size.pillButton
                                tonal: false
                                icon: "close"
                                iconSize: Theme.icon.small
                                iconColor: Theme.colors.textMuted
                                onClicked: QuickShare.removeFile(file.modelData.path)
                            }
                        }
                    }
                }
            }

            // Click to pick files; dropping them anywhere on the tab works too.
            Item {
                Layout.fillWidth: true
                implicitHeight: Theme.size.pillButton + 2 * Theme.spacing.sm

                Rectangle {
                    anchors.fill: parent
                    radius: Theme.radius.card
                    color: dropTarget.containsDrag ? Theme.colors.selected : "transparent"

                    Behavior on color {
                        ColorAnim {}
                    }
                }

                Shape {
                    id: dashes

                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer

                    ShapePath {
                        strokeWidth: Theme.size.dropBorder
                        strokeColor: dropTarget.containsDrag ? Theme.colors.primary : Theme.colors.dropOutline
                        strokeStyle: ShapePath.DashLine
                        dashPattern: [Theme.size.dropDash, Theme.size.dropDash]
                        fillColor: "transparent"

                        PathRectangle {
                            x: Theme.size.dropBorder / 2
                            y: Theme.size.dropBorder / 2
                            width: dashes.width - Theme.size.dropBorder
                            height: dashes.height - Theme.size.dropBorder
                            radius: Theme.radius.card
                        }
                    }
                }

                Clickable {
                    radius: Theme.radius.card
                    onClicked: QuickShare.pickFiles()
                }

                RowLayout {
                    anchors.centerIn: parent
                    spacing: Theme.spacing.xs

                    MaterialIcon {
                        icon: "add"
                        color: Theme.colors.textMuted
                    }

                    StyledText {
                        text: I18n.tr("Choose files …")
                        color: Theme.colors.textMuted
                    }
                }
            }

            SectionHeader {
                topPadding: Theme.spacing.sm
                text: QuickShare.devices.length > 0 ? I18n.tr("Nearby devices") : I18n.tr("Searching for devices…")
            }

            Card {
                Layout.fillWidth: true
                visible: QuickShare.devices.length > 0
                implicitHeight: devices.implicitHeight + 2 * Theme.spacing.xs

                Column {
                    id: devices

                    anchors.fill: parent
                    anchors.margins: Theme.spacing.xs

                    Repeater {
                        model: ScriptModel {
                            values: QuickShare.devices
                        }

                        ListRow {
                            id: device

                            required property var modelData
                            readonly property var transfer: QuickShare.deviceTransfer(modelData.id)
                            readonly property bool busy: transfer !== null && !QuickShare.finalStates.includes(transfer.state)

                            width: parent.width
                            clickable: false
                            icon: QuickShare.deviceIcon(modelData.kind)
                            title: QuickShare.deviceName(modelData.name, modelData.kind)
                            subtitle: root.status(transfer)

                            PillButton {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: !device.busy
                                enabled: root.hasFiles
                                style: "filled"
                                text: I18n.tr("Send")
                                onClicked: QuickShare.sendTo(device.modelData.id)
                            }

                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: device.busy && device.transfer.state === "transferring"
                                implicitWidth: Theme.size.progressBarWidth
                                implicitHeight: Theme.size.progressBarHeight
                                radius: height / 2
                                color: Theme.colors.surfaceHighest

                                Rectangle {
                                    height: parent.height
                                    width: Math.max(height, parent.width * (device.transfer && device.transfer.total > 0 ? Math.min(1, device.transfer.done / device.transfer.total) : 0))
                                    radius: height / 2
                                    color: Theme.colors.primary

                                    Behavior on width {
                                        Anim {}
                                    }
                                }
                            }

                            IconButton {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: device.busy
                                implicitWidth: Theme.size.pillButton
                                implicitHeight: Theme.size.pillButton
                                tonal: false
                                icon: "close"
                                iconSize: Theme.icon.small
                                iconColor: Theme.colors.textMuted
                                onClicked: QuickShare.cancel(device.transfer.id)
                            }
                        }
                    }
                }
            }

            StyledText {
                Layout.fillWidth: true
                Layout.topMargin: Theme.spacing.xs
                text: I18n.tr("On the phone, Quick Share must be set to “Everyone”.")
                color: Theme.colors.textMuted
                font.pixelSize: Theme.font.small
                wrapMode: Text.WordWrap
            }

            StyledText {
                Layout.fillWidth: true
                // Before the backend's hello its BLE state is not known yet.
                visible: QuickShare.connected && !QuickShare.ble
                text: I18n.tr("Bluetooth is off – phones may not be found.")
                color: Theme.colors.textMuted
                font.pixelSize: Theme.font.small
                wrapMode: Text.WordWrap
            }
        }
    }

    DropArea {
        id: dropTarget

        anchors.fill: parent
        onEntered: drag => drag.accepted = drag.hasUrls
        onDropped: drop => {
            const paths = drop.urls.map(u => u.toString()).filter(u => u.startsWith("file://")).map(u => decodeURIComponent(u.slice("file://".length)));
            QuickShare.addFiles(paths);
            drop.acceptProposedAction();
        }
    }
}
