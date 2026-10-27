pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io

// Settings shared with the vela launcher (~/.config/vela/config.toml),
// streamed as JSON lines by `vela shell-config --watch`: sanitized values
// and a palette derived from the vela theme. Until the first line arrives
// (or without vela) the defaults below, equal to vela's, apply.
Singleton {
    id: root

    property bool loaded: false

    property var colors: ({
            background: "#18181e",
            text: "#ececf4",
            textMuted: "#9696a8",
            textDisabled: "#5d5d6a",
            tint: "#ffffff",
            primary: "#7aa2f7",
            textOnPrimary: "#101014",
            primaryMuted: "#3a486a",
            error: "#f2b8b5",
            textOnError: "#601410",
            errorSurface: "#372e33"
        })
    property var idle: ({
            suspend: true,
            suspendAfterMin: 30
        })
    property var appearance: ({
            light: false,
            radius: 22,
            fontScale: 1,
            opacity: 0.86,
            surfaceOpacity: 0.06,
            animationScale: 1,
            backdropDim: 0.18,
            fontFamily: "Adwaita Sans"
        })
    property var panel: ({
            width: 420,
            closeOnFocusLoss: true,
            backdrop: false,
            backdropBlur: true,
            backdropLayers: 1,
            backdropLayerAlpha: 0.18,
            popupTimeoutMs: 5000,
            popupMaxVisible: 4,
            criticalPopupsStay: true,
            groupCollapsedCount: 2,
            compactNotifications: true,
            workspaceOsd: true,
            nightLightTemperature: 4000,
            clockCentered: false,
            clockOnBackdrop: false,
            clockFont: "",
            backdropClockSize: 128,
            mediaPlayer: true,
            mediaPlayerAny: false,
            mediaPlayerPosition: "tiles",
            mediaPlayerCover: true,
            mediaPlayerCoverBackground: false,
            mediaPlayerProgress: true,
            mediaPlayerShuffleRepeat: true,
            mediaPlayerScale: 1,
            mediaPlayerCoverBlur: 0.75,
            mediaPlayerButtonScale: 1,
            mediaPlayerButtonBackground: true,
            claudeUsage: true,
            claudeUsageSubtle: false,
            claudeUsageOnlyDefault: false,
            claudeUsageHidden: [],
            updatesTile: true,
            quickshareTile: false
        })
    // Quick Share: whether the component is installed, and the visible name.
    property var quickshare: ({
            available: false,
            name: "Linux"
        })
    // Pulse, the task manager.
    property var pulse: ({
            smoothGraphs: true,
            startPage: "overview",
            rangeSecs: 60,
            confirmEnd: false,
            heatMap: true,
            showKernel: false,
            sensorView: "list",
            cpuTemp: false,
            throttleTint: true,
            intervalMs: 1000,
            record: true,
            claude: true,
            keys: {
                force: "k",
                end: "g",
                restart: "r",
                pause: "p",
                efficiency: "e"
            }
        })

    function apply(line: string): void {
        try {
            const d = JSON.parse(line);
            idle = d.idle;
            colors = d.colors;
            appearance = d.appearance;
            panel = d.panel;
            if (d.pulse)
                pulse = d.pulse;
            if (d.quickshare)
                quickshare = d.quickshare;
            loaded = true;
        } catch (e) {
            console.warn("VelaConfig: unreadable settings line:", e);
        }
    }

    Process {
        id: proc

        command: [Quickshell.env("VELA_BIN") || "vela", "shell-config", "--watch"]
        running: true
        stdout: SplitParser {
            onRead: line => root.apply(line)
        }
        stderr: SplitParser {
            onRead: line => console.warn(line)
        }
        onExited: exitCode => {
            console.warn("VelaConfig: vela shell-config exited with", exitCode, "- retrying");
            retry.start();
        }
    }

    Timer {
        id: retry

        interval: 3000
        onTriggered: proc.running = true
    }
}
