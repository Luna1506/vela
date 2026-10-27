//@ pragma IconTheme Papirus-Dark

import QtQuick
import Quickshell
import qs
import Quickshell.Hyprland
import Quickshell.Io
import qs.services
import qs.panel
import qs.notifications
import qs.osd
import qs.quickshare

ShellRoot {
    // Singletons are created lazily; these must run from the start
    // (notification daemon, restoring the night light).
    Component.onCompleted: {
        Notifications.count;
        NightLight.enabled;
    }

    ControlCenter {
        id: controlCenter
    }

    ClickCatcher {
        id: clickCatcher
    }

    // Closes the panel when focus is taken away explicitly: a click on a
    // surface outside the grab, or another layer grabbing the keyboard
    // (e.g. a launcher). Merely hovering another monitor doesn't count.
    HyprlandFocusGrab {
        windows: [controlCenter, ...clickCatcher.instances]
        active: ShellState.panelOpen && Config.closeOnFocusLoss
        onCleared: ShellState.closePanel()
    }

    Backdrop {}

    NotificationPopups {}

    // Asks about every incoming Quick Share offer, panel open or not.
    LazyLoader {
        active: Config.quickShareAvailable

        IncomingWindow {}
    }

    WorkspaceOsd {}

    // Hyprland: bind = SUPER, N, global, quickshell:panelToggle
    GlobalShortcut {
        name: "panelToggle"
        description: I18n.tr("Toggle the control center")
        onPressed: ShellState.togglePanel()
    }

    // qs -c luna ipc call panel toggle|open|close|detail <wifi|bluetooth|audio>
    IpcHandler {
        target: "panel"

        function toggle(): void {
            ShellState.togglePanel();
        }

        function open(): void {
            ShellState.openPanel();
        }

        function close(): void {
            ShellState.closePanel();
        }

        function detail(name: string): void {
            ShellState.openDetail(name);
        }
    }

    // qs -c luna ipc call toggle dnd|nightLight
    IpcHandler {
        target: "toggle"

        function dnd(): void {
            Notifications.toggleDnd();
        }

        function nightLight(): void {
            NightLight.toggle();
        }
    }

    // qs -p <dir> ipc call notifications dismissApp <app> | clearAll
    IpcHandler {
        target: "notifications"

        function dismissApp(app: string): void {
            Notifications.dismissApp(app);
        }

        function clearAll(): void {
            Notifications.clearAll();
        }
    }
}
