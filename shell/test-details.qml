// Every detail view of the panel can be created (they load lazily in the
// shell, so errors would only show on click).
//   qs -p test-details.qml   (scripts/shell-test.sh); prints PASS or FAIL
import QtQuick
import Quickshell
import qs.details

ShellRoot {
    Item {
        id: host

        width: 380
        height: 600
    }

    Component.onCompleted: {
        const names = ["details/WifiDetail", "details/BluetoothDetail", "details/AudioDetail", "details/PowerDetail", "details/QuickShareDetail", "panel/ClaudeUsageSection"];
        const failed = names.filter(n => {
            const c = Qt.createComponent(n + ".qml");
            const o = c.status === Component.Ready ? c.createObject(host) : null;
            if (!o)
                console.log(n + ": " + c.errorString());
            return !o;
        });
        console.log((failed.length === 0 ? "PASS " : "FAIL ") + JSON.stringify(failed));
        Qt.callLater(Qt.quit);
    }
}
