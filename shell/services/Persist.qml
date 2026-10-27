pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io

// Small JSON state file (~/.local/state/quickshell/...) for toggles that
// should survive restarts.
Singleton {
    id: root

    property alias doNotDisturb: adapter.doNotDisturb
    property alias nightLight: adapter.nightLight
    property alias quickShare: adapter.quickShare
    property alias quickShareDir: adapter.quickShareDir

    FileView {
        path: Quickshell.statePath("state.json")
        printErrors: false
        onAdapterUpdated: writeAdapter()
        onLoadFailed: error => {
            if (error === FileViewError.FileNotFound)
                writeAdapter();
            else
                console.warn("Persist: failed to read state file:", FileViewError.toString(error));
        }

        JsonAdapter {
            id: adapter

            property bool doNotDisturb: false
            property bool nightLight: false
            property bool quickShare: false
            property string quickShareDir: ""
        }
    }
}
