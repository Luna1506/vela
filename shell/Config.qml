pragma Singleton

import QtQuick
import Quickshell

// Behavior settings (timings, limits, commands). Visuals live in Theme.qml.
Singleton {
    // Workspace OSD
    readonly property int osdTimeout: 1200
    readonly property int osdMinWorkspaces: 5
    readonly property int osdMaxWorkspaces: 10

    // Notifications
    readonly property var vp: VelaConfig.panel

    // Control center (vela settings → Panel)
    readonly property bool closeOnFocusLoss: vp.closeOnFocusLoss
    readonly property bool panelBackdrop: vp.backdrop
    readonly property bool panelBackdropBlur: vp.backdropBlur
    readonly property int panelBackdropLayers: vp.backdropLayers
    readonly property real panelBackdropLayerAlpha: vp.backdropLayerAlpha
    readonly property bool workspaceOsd: vp.workspaceOsd
    readonly property bool clockCentered: vp.clockCentered
    // Already false without a blurred backdrop (src/shell.rs).
    readonly property bool clockOnBackdrop: vp.clockOnBackdrop ?? false
    // Mini player; the position is already "tiles" unless the clock is on the blur.
    readonly property bool mediaPlayer: vp.mediaPlayer ?? true
    readonly property bool mediaPlayerAny: vp.mediaPlayerAny ?? false
    readonly property string mediaPlayerPosition: vp.mediaPlayerPosition ?? "tiles"
    readonly property bool mediaPlayerCover: vp.mediaPlayerCover ?? true
    readonly property bool mediaPlayerCoverBackground: vp.mediaPlayerCoverBackground ?? false
    readonly property bool mediaPlayerProgress: vp.mediaPlayerProgress ?? true
    readonly property bool mediaPlayerShuffleRepeat: vp.mediaPlayerShuffleRepeat ?? true
    readonly property real mediaPlayerScale: vp.mediaPlayerScale ?? 1
    readonly property real mediaPlayerCoverBlur: vp.mediaPlayerCoverBlur ?? 0.75
    readonly property real mediaPlayerButtonScale: vp.mediaPlayerButtonScale ?? 1
    readonly property bool mediaPlayerButtonBackground: vp.mediaPlayerButtonBackground ?? true
    readonly property bool claudeUsage: vp.claudeUsage
    readonly property bool claudeUsageSubtle: vp.claudeUsageSubtle
    readonly property bool claudeUsageOnlyDefault: vp.claudeUsageOnlyDefault
    readonly property var claudeUsageHidden: vp.claudeUsageHidden
    readonly property bool updatesTile: vp.updatesTile ?? true
    readonly property bool quickshareTile: vp.quickshareTile ?? false
    readonly property bool quickShareAvailable: VelaConfig.quickshare.available ?? false
    readonly property string quickShareName: VelaConfig.quickshare.name ?? ""
    readonly property bool compactNotifications: vp.compactNotifications

    // Compact notifications unfold after the pointer rests this long (ms).
    readonly property int hoverExpandDelay: 600

    readonly property int popupTimeout: vp.popupTimeoutMs
    // Upper bound for app-requested timeouts (ms).
    readonly property int popupMaxTimeout: 30000
    readonly property int popupMaxVisible: vp.popupMaxVisible
    readonly property bool criticalPopupsStay: vp.criticalPopupsStay
    // Notifications shown per app group before "Show more".
    readonly property int groupCollapsedCount: vp.groupCollapsedCount
    // Refresh rate of "5 min ago" labels while visible (ms).
    readonly property int relativeTimeInterval: 30000

    // Duration factor of all animations: 1 = normal, 2 = twice as slow,
    // 0 = off (vela's animations switch and speed).
    readonly property real animationScale: VelaConfig.appearance.animationScale

    // Sliders: volume/brightness change per mouse wheel notch.
    readonly property real sliderWheelStep: 0.05

    // Clock and date follow the system locale (LC_TIME), see I18n.qml.
    readonly property var locale: I18n.timeLocale
    readonly property string timeFormat: locale.timeFormat(Locale.ShortFormat)
    readonly property string dateFormat: I18n.withoutYear(locale.dateFormat(Locale.LongFormat))
    readonly property string shortDateFormat: I18n.withoutYear(locale.dateFormat(Locale.ShortFormat))

    // Night light color temperature in Kelvin (hyprsunset).
    readonly property int nightLightTemperature: vp.nightLightTemperature

    // Session actions
    readonly property var lockCommand: ["hyprlock"]
    readonly property var rebootCommand: ["systemctl", "reboot"]
    readonly property var poweroffCommand: ["systemctl", "poweroff"]
}
