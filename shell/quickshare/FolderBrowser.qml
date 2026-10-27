import QtQuick
import QtQuick.Layouts
import Quickshell
import qs
import qs.components
import qs.services

// Picks the folder an incoming transfer is saved to: a path bar, the
// subfolders of `dir` (a click goes into one) and a field for a new folder.
// The backend lists and creates the folders (QuickShare.listDirs/makeDir).
ColumnLayout {
    id: root

    // Where it opens; "" is the download folder.
    property string startDir
    // The folder that is open (and saved to).
    property string dir: startDir
    property var dirs: []
    property bool loading: false
    // Reading the folder failed and there is no way up.
    property string error: ""
    // Why another folder than the one asked for is shown.
    property string hint: ""
    // The new-folder field is open.
    property bool creating: false
    property alias folderName: field.text
    property string fieldError: ""

    // The new-folder field closed; the keyboard can go back to the window.
    signal fieldClosed

    readonly property string home: Quickshell.env("HOME") ?? ""
    readonly property bool canGoUp: dir !== "" && dir !== "/"
    readonly property string saveText: dir !== "" ? I18n.tr("Saving to %1", shortPath(dir)) : ""

    // [{ home, label, path }]: home as an icon (or "/"), then each folder.
    readonly property var crumbs: {
        if (dir === "")
            return [];
        const inHome = home !== "" && home !== "/" && (dir === home || dir.startsWith(home + "/"));
        const out = [inHome ? {
                home: true,
                label: "",
                path: home
            } : {
                home: false,
                label: "/",
                path: "/"
            }];
        let path = inHome ? home : "";
        for (const part of dir.slice(inHome ? home.length : 0).split("/").filter(p => p !== "")) {
            path += "/" + part;
            out.push({
                home: false,
                label: part,
                path: path
            });
        }
        return out;
    }

    // Answers to an older listing are dropped.
    property int serial: 0

    function shortPath(path: string): string {
        if (home !== "" && home !== "/" && (path === home || path.startsWith(home + "/")))
            return "~" + path.slice(home.length);
        return path;
    }

    function baseName(path: string): string {
        return path.split("/").filter(p => p !== "").pop() ?? path;
    }

    function parentOf(path: string): string {
        const i = path.lastIndexOf("/");
        return i > 0 ? path.slice(0, i) : "/";
    }

    // Opens a folder the user chose.
    function open(path: string): void {
        hint = "";
        load(path);
    }

    function goUp(): void {
        if (canGoUp)
            open(parentOf(dir));
    }

    function load(path: string): void {
        dir = path;
        loading = true;
        error = "";
        const n = ++serial;
        QuickShare.listDirs(path, reply => {
            if (n === root.serial)
                root.listed(reply);
        });
    }

    // A `dirs` reply: { dir, parent, dirs, error }, or just { error }.
    function listed(reply: var): void {
        loading = false;
        if (reply.error) {
            // Gone or unreadable: one level up, until a folder can be read.
            const asked = reply.dir || dir;
            if (reply.parent && reply.parent !== asked) {
                hint = I18n.tr("“%1” can't be opened – showing the folder above.", baseName(asked));
                load(reply.parent);
                return;
            }
            dirs = [];
            error = reply.error;
            return;
        }
        dir = reply.dir;
        dirs = reply.dirs ?? [];
        error = "";
    }

    function startNewFolder(): void {
        folderName = "";
        fieldError = "";
        creating = true;
        field.input.forceActiveFocus();
    }

    function cancelNewFolder(): void {
        creating = false;
        fieldError = "";
        fieldClosed();
    }

    function createFolder(): void {
        const name = folderName.trim();
        if (name === "" || name === "." || name === ".." || name.includes("/") || /[\x00-\x1f\x7f]/.test(name)) {
            fieldError = I18n.tr("Invalid name");
            return;
        }
        if (dirs.includes(name)) {
            fieldError = I18n.tr("Already exists");
            return;
        }
        QuickShare.makeDir(dir, name, reply => root.made(reply));
    }

    // A `mkdir` reply: { path, error }. The new folder is opened.
    function made(reply: var): void {
        if (reply.error) {
            if (/exist|os error 17/i.test(reply.error))
                fieldError = I18n.tr("Already exists");
            else if (/not a valid/i.test(reply.error))
                fieldError = I18n.tr("Invalid name");
            else
                fieldError = reply.error;
            return;
        }
        creating = false;
        fieldError = "";
        open(reply.path);
        fieldClosed();
    }

    // Keys of the new-folder field: Enter creates, Esc closes only the field.
    function handleKey(event: var): void {
        if (!creating)
            return;
        if (event.key === Qt.Key_Escape)
            cancelNewFolder();
        else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
            createFolder();
        else
            return;
        event.accepted = true;
    }

    Component.onCompleted: open(startDir)

    spacing: Theme.spacing.sm

    // Path bar, up, new folder.
    RowLayout {
        Layout.fillWidth: true
        spacing: Theme.spacing.xs

        Flickable {
            id: crumbBar

            Layout.fillWidth: true
            implicitHeight: Theme.size.pillButton
            contentWidth: crumbRow.implicitWidth
            contentHeight: height
            // The end of a long path stays in view.
            contentX: Math.max(0, contentWidth - width)
            flickableDirection: Flickable.HorizontalFlick
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Row {
                id: crumbRow

                height: parent.height

                Repeater {
                    model: root.crumbs

                    Row {
                        id: crumb

                        required property var modelData
                        required property int index
                        readonly property bool last: index === root.crumbs.length - 1

                        height: crumbRow.height

                        MaterialIcon {
                            visible: crumb.index > 0
                            anchors.verticalCenter: parent.verticalCenter
                            icon: "chevron_right"
                            size: Theme.icon.small
                            color: Theme.colors.textDisabled
                        }

                        Item {
                            width: (crumb.modelData.home ? homeIcon.implicitWidth : crumbText.implicitWidth) + 2 * Theme.spacing.xs
                            height: parent.height

                            Clickable {
                                enabled: !crumb.last
                                radius: Theme.radius.small
                                onClicked: root.open(crumb.modelData.path)
                            }

                            MaterialIcon {
                                id: homeIcon

                                visible: crumb.modelData.home
                                anchors.centerIn: parent
                                icon: "home"
                                color: crumb.last ? Theme.colors.text : Theme.colors.textMuted
                            }

                            StyledText {
                                id: crumbText

                                visible: !crumb.modelData.home
                                anchors.centerIn: parent
                                text: crumb.modelData.label
                                color: crumb.last ? Theme.colors.text : Theme.colors.textMuted
                                font.weight: crumb.last ? Theme.font.weightMedium : Theme.font.weightNormal
                            }
                        }
                    }
                }
            }
        }

        IconButton {
            icon: "arrow_upward"
            enabled: root.canGoUp
            onClicked: root.goUp()
        }

        PillButton {
            icon: "create_new_folder"
            text: I18n.tr("New folder")
            onClicked: root.creating ? field.input.forceActiveFocus() : root.startNewFolder()
        }
    }

    Card {
        Layout.fillWidth: true
        implicitHeight: Theme.size.folderListHeight

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Theme.spacing.xs
            spacing: 0

            // The new folder, on top of the list.
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing.sm
                visible: root.creating
                spacing: Theme.spacing.sm

                MaterialIcon {
                    icon: "create_new_folder"
                    color: Theme.colors.primary
                }

                TextField {
                    id: field

                    Layout.fillWidth: true
                    placeholder: I18n.tr("Folder name")
                    input.Keys.onPressed: event => root.handleKey(event)
                }

                IconButton {
                    icon: "check"
                    onClicked: root.createFolder()
                }
            }

            StyledText {
                Layout.fillWidth: true
                Layout.leftMargin: Theme.spacing.sm
                Layout.bottomMargin: Theme.spacing.xs
                visible: root.creating && root.fieldError !== ""
                text: root.fieldError
                color: Theme.colors.error
                font.pixelSize: Theme.font.small
                wrapMode: Text.WordWrap
            }

            ListView {
                id: list

                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                model: root.dirs

                delegate: Item {
                    id: row

                    required property string modelData

                    width: list.width
                    implicitHeight: Theme.size.iconButton + Theme.spacing.xs

                    Clickable {
                        radius: Theme.radius.small
                        onClicked: root.open(root.dir === "/" ? "/" + row.modelData : root.dir + "/" + row.modelData)
                    }

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: Theme.spacing.sm
                        anchors.rightMargin: Theme.spacing.sm
                        spacing: Theme.spacing.md

                        MaterialIcon {
                            icon: "folder"
                        }

                        StyledText {
                            Layout.fillWidth: true
                            text: row.modelData
                        }

                        MaterialIcon {
                            icon: "chevron_right"
                            color: Theme.colors.textMuted
                        }
                    }
                }

                StyledText {
                    anchors.centerIn: parent
                    width: parent.width - 2 * Theme.spacing.lg
                    visible: !root.loading && list.count === 0
                    horizontalAlignment: Text.AlignHCenter
                    text: root.error !== "" ? root.error : I18n.tr("No subfolders")
                    color: root.error !== "" ? Theme.colors.error : Theme.colors.textMuted
                    wrapMode: Text.WordWrap
                }
            }
        }
    }

    StyledText {
        Layout.fillWidth: true
        visible: root.hint !== ""
        text: root.hint
        color: Theme.colors.textMuted
        font.pixelSize: Theme.font.small
        wrapMode: Text.WordWrap
    }

    StyledText {
        Layout.fillWidth: true
        visible: text !== ""
        text: root.saveText
        color: Theme.colors.textMuted
        font.pixelSize: Theme.font.small
        elide: Text.ElideMiddle
    }
}
