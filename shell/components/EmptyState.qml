import QtQuick
import QtQuick.Layouts
import qs

// Centered icon + title + hint, for empty lists and missing hardware.
ColumnLayout {
    id: root

    property string icon
    property string title
    property string subtitle

    spacing: Theme.spacing.sm

    Rectangle {
        Layout.alignment: Qt.AlignHCenter
        implicitWidth: Theme.icon.huge + 2 * Theme.spacing.lg
        implicitHeight: implicitWidth
        radius: width / 2
        color: Theme.colors.surfaceHigh

        MaterialIcon {
            anchors.centerIn: parent
            icon: root.icon
            size: Theme.icon.huge
            color: Theme.colors.textMuted
        }
    }

    // Full width (centred text): a nested layout is only as wide as its
    // widest child allows, and the view's fillWidth should reach it.
    StyledText {
        Layout.fillWidth: true
        Layout.topMargin: Theme.spacing.xs
        horizontalAlignment: Text.AlignHCenter
        text: root.title
        font.pixelSize: Theme.font.title
        font.weight: Theme.font.weightMedium
    }

    SwapText {
        Layout.fillWidth: true
        visible: root.subtitle !== ""
        value: root.subtitle
        color: Theme.colors.textMuted
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
    }
}
