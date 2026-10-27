import QtQuick
import QtQuick.Layouts
import qs

// A row of equal buttons of which one is selected, like a tab bar: the
// accent selection glides to the chosen one. `model`: [{ key, icon, text }].
Rectangle {
    id: root

    property var model: []
    property string current
    readonly property int currentIndex: model.findIndex(m => m.key === current)

    signal selected(string key)

    implicitHeight: Theme.size.pillButton + 2 * Theme.spacing.xs
    implicitWidth: Theme.size.controlWidth
    radius: Theme.radius.card
    color: Theme.colors.tile

    Rectangle {
        id: highlight

        // count: re-evaluated once the segments exist.
        readonly property Item target: repeater.count > 0 && root.currentIndex >= 0 ? repeater.itemAt(root.currentIndex) : null
        property bool placed: false

        visible: target !== null
        x: row.x + (target?.x ?? 0)
        y: row.y
        width: target?.width ?? 0
        height: row.height
        radius: Theme.radius.small
        color: Theme.colors.selected
        border.width: Theme.size.border
        border.color: Theme.colors.selectedRing

        onTargetChanged: {
            if (target && !placed)
                Qt.callLater(() => highlight.placed = true);
        }

        Behavior on x {
            enabled: highlight.placed

            SpringAnim {
                duration: Theme.anim.normal
            }
        }
    }

    RowLayout {
        id: row

        anchors.fill: parent
        anchors.margins: Theme.spacing.xs
        spacing: Theme.spacing.xs

        Repeater {
            id: repeater

            model: root.model

            Item {
                id: segment

                required property var modelData
                required property int index
                readonly property bool isCurrent: index === root.currentIndex

                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.preferredWidth: 1

                Clickable {
                    radius: Theme.radius.small
                    onClicked: root.selected(segment.modelData.key)
                }

                RowLayout {
                    anchors.centerIn: parent
                    spacing: Theme.spacing.xs

                    MaterialIcon {
                        visible: (segment.modelData.icon ?? "") !== ""
                        icon: segment.modelData.icon ?? ""
                        size: Theme.icon.small
                        color: segment.isCurrent ? Theme.colors.text : Theme.colors.textMuted
                    }

                    StyledText {
                        text: segment.modelData.text
                        color: segment.isCurrent ? Theme.colors.text : Theme.colors.textMuted
                        font.weight: segment.isCurrent ? Theme.font.weightMedium : Theme.font.weightNormal

                        Behavior on color {
                            ColorAnim {}
                        }
                    }
                }
            }
        }
    }
}
