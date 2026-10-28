import QtQuick
import QtQuick.Layouts
import Quickshell.Io
import qs
import qs.components
import qs.services

// Receive tab of the Quick Share view: this PC is visible while Quick Share
// is on; here it gets its name. Offers themselves open their own window.
ColumnLayout {
    id: root

    readonly property int circle: Theme.icon.huge + 2 * Theme.spacing.lg
    // The name phones see when none is set (like the backend: the hostname).
    readonly property string systemName: hostname.text().trim() || "Linux"
    // 0..1, one wave of the pulse around the circle.
    property real wave: 0

    objectName: "receive"
    spacing: Theme.spacing.sm

    NumberAnimation on wave {
        from: 0
        to: 1
        duration: Theme.anim.pulse
        loops: Animation.Infinite
        running: root.visible && Theme.anim.pulse > 0
    }

    FileView {
        id: hostname

        path: "/proc/sys/kernel/hostname"
        blockLoading: true
        printErrors: false
    }

    Item {
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacing.sm
        implicitHeight: root.circle + 2 * Theme.spacing.xl

        // Two rings, half a wave apart, grow out of the circle and fade.
        Repeater {
            model: 2

            Rectangle {
                required property int index
                readonly property real phase: (root.wave + index / 2) % 1

                anchors.centerIn: parent
                width: root.circle
                height: width
                radius: width / 2
                color: Theme.colors.accentChip
                scale: 1 + phase * 2 * Theme.spacing.xl / root.circle
                opacity: 1 - phase
            }
        }

        Rectangle {
            anchors.centerIn: parent
            width: root.circle
            height: width
            radius: width / 2
            color: Theme.colors.accentChip

            MaterialIcon {
                anchors.centerIn: parent
                icon: "laptop"
                size: Theme.icon.huge
                color: Theme.colors.primary
            }
        }
    }

    StyledText {
        Layout.fillWidth: true
        horizontalAlignment: Text.AlignHCenter
        text: I18n.tr("Visible to nearby devices")
        font.pixelSize: Theme.font.title
        font.weight: Theme.font.weightMedium
    }

    StyledText {
        Layout.fillWidth: true
        horizontalAlignment: Text.AlignHCenter
        text: I18n.tr("On the phone, choose Quick Share and then “%1”", QuickShare.name || root.systemName)
        color: Theme.colors.textMuted
        wrapMode: Text.WordWrap
    }

    SectionHeader {
        topPadding: Theme.spacing.md
        text: I18n.tr("Name")
    }

    TextField {
        Layout.fillWidth: true
        // The config hands over the name in use: the hostname means none is set.
        text: Config.quickShareName !== root.systemName ? Config.quickShareName : ""
        placeholder: root.systemName
        onAccepted: {
            QuickShare.setName(text.trim());
            input.focus = false;
        }
    }

    StyledText {
        Layout.fillWidth: true
        text: I18n.tr("Leave empty for the system name.")
        color: Theme.colors.textMuted
        font.pixelSize: Theme.font.small
        wrapMode: Text.WordWrap
    }

    StyledText {
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacing.md
        text: I18n.tr("Phone and PC must be on the same Wi-Fi.")
        color: Theme.colors.textMuted
        font.pixelSize: Theme.font.small
        wrapMode: Text.WordWrap
    }

    // Always shown: receiving with "Share with Apple devices" on is untested.
    StyledText {
        objectName: "appleHint"
        Layout.fillWidth: true
        text: I18n.tr("Phone doesn't find this PC? Turn off “Share with Apple devices” in Quick Share on the phone.")
        color: Theme.colors.textMuted
        font.pixelSize: Theme.font.small
        wrapMode: Text.WordWrap
    }
}
