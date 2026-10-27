import QtQuick
import QtQuick.Layouts
import qs
import qs.components
import qs.services

// Icon = toggle, rest of the tile = detail view (where one exists).
GridLayout {
    columns: 2
    rowSpacing: Theme.spacing.sm
    columnSpacing: Theme.spacing.sm

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: NetworkState.icon
        title: "Internet"
        subtitle: NetworkState.status
        active: NetworkState.online || NetworkState.usable
        hasDetail: true
        enabled: NetworkState.wifiAvailable || NetworkState.wired !== null
        onToggled: NetworkState.toggleWifi()
        onDetailRequested: ShellState.openDetail("wifi")
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: !BluetoothState.enabled ? "bluetooth_disabled" : BluetoothState.connected.length > 0 ? "bluetooth_connected" : "bluetooth"
        title: "Bluetooth"
        subtitle: BluetoothState.status
        active: BluetoothState.enabled
        hasDetail: true
        enabled: BluetoothState.available
        onToggled: BluetoothState.toggle()
        onDetailRequested: ShellState.openDetail("bluetooth")
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: Audio.muted ? "volume_off" : "volume_up"
        title: I18n.tr("Sound")
        subtitle: Audio.muted ? I18n.tr("Muted") : Audio.displayName(Audio.sink)
        active: Audio.sink !== null && !Audio.muted
        hasDetail: true
        onToggled: Audio.toggleMute(Audio.sink)
        onDetailRequested: ShellState.openDetail("audio")
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: PowerMode.icon(PowerMode.profile)
        title: I18n.tr("Power mode")
        subtitle: PowerMode.available ? PowerMode.label(PowerMode.profile) : I18n.tr("Not available")
        // Highlighted when not on the default (balanced) profile.
        active: PowerMode.available && PowerMode.profile !== "balanced"
        enabled: PowerMode.available
        hasDetail: true
        onToggled: PowerMode.cycle()
        onDetailRequested: ShellState.openDetail("power")
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: "bedtime"
        title: I18n.tr("Night light")
        subtitle: NightLight.enabled ? Config.nightLightTemperature + " K" : I18n.tr("Off")
        active: NightLight.enabled
        onToggled: NightLight.toggle()
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.preferredWidth: 1
        icon: Notifications.dnd ? "do_not_disturb_on" : "do_not_disturb_off"
        title: I18n.tr("Do not disturb")
        subtitle: Notifications.dnd ? I18n.tr("On") : I18n.tr("Off")
        active: Notifications.dnd
        onToggled: Notifications.toggleDnd()
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.columnSpan: 2
        visible: Config.updatesTile
        icon: SystemUpdates.icon(SystemUpdates.status)
        title: I18n.tr("Updates")
        subtitle: SystemUpdates.subtitle(SystemUpdates.status)
        active: SystemUpdates.highlighted(SystemUpdates.status)
        hasDetail: true
        onToggled: SystemUpdates.openSettings()
        onDetailRequested: SystemUpdates.openSettings()
    }

    ToggleTile {
        Layout.fillWidth: true
        Layout.columnSpan: 2
        visible: Config.quickshareTile
        icon: "share"
        title: I18n.tr("Quick Share")
        subtitle: QuickShare.subtitle()
        active: QuickShare.enabled
        hasDetail: true
        onToggled: QuickShare.toggle()
        onDetailRequested: ShellState.openDetail("quickshare")
    }
}
