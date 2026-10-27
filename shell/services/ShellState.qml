pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Hyprland

// UI state of the control center: whether it is open, on which screen, and
// which detail view is shown.
Singleton {
    id: root

    property bool panelOpen: false
    property ShellScreen panelScreen: null
    // "" (none) or one of `details`
    property string detail: ""
    // Tab of the Quick Share view: "send" or "receive".
    property string quickShareTab: "receive"

    function focusedScreen(): ShellScreen {
        const name = Hyprland.focusedMonitor?.name;
        return Quickshell.screens.find(s => s.name === name) ?? Quickshell.screens[0] ?? null;
    }

    function openPanel(): void {
        if (panelOpen)
            return;
        panelScreen = focusedScreen();
        if (!panelScreen)
            return;
        detail = "";
        panelOpen = true;
    }

    function closePanel(): void {
        panelOpen = false;
        detail = "";
    }

    function togglePanel(): void {
        panelOpen ? closePanel() : openPanel();
    }

    readonly property var details: ["wifi", "bluetooth", "audio", "power", "quickshare"]

    function openDetail(name: string): void {
        if (!details.includes(name)) {
            console.warn("ShellState: unknown detail view", name);
            return;
        }
        openPanel();
        detail = detail === name ? "" : name;
    }

    function closeDetail(): void {
        detail = "";
    }

    // Escape: close the detail view first, then the panel.
    function back(): void {
        detail !== "" ? closeDetail() : closePanel();
    }

    // Close the panel if its monitor gets unplugged.
    Connections {
        target: Quickshell

        function onScreensChanged(): void {
            // Clearing the screen also unmaps the window right away, so it
            // doesn't reappear on another output for the close animation.
            if (root.panelScreen && !Quickshell.screens.includes(root.panelScreen)) {
                root.closePanel();
                root.panelScreen = null;
            }
        }
    }
}
