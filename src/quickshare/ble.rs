//! The BLE beacon that makes phones list us while sending.
//!
//! A phone in Quick Share's "Everyone" mode only announces itself over mDNS
//! while it sees a Quick Share BLE advertisement nearby. [`start`] registers
//! one with BlueZ over D-Bus: service UUID `0xFE2C` with fixed service data
//! and a random tail. It stays on the air until the [`Beacon`] is dropped.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The Quick Share (Nearby) service, as a 128-bit UUID.
const SERVICE_UUID: &str = "0000fe2c-0000-1000-8000-00805f9b34fb";
/// Where our `org.bluez.LEAdvertisement1` object lives on our connection.
const ADV_PATH: &str = "/org/vela/quickshare/adv0";
pub(super) const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
pub(super) const MANAGER: &str = "org.bluez.LEAdvertisingManager1";
/// The longest any D-Bus call may take, so a hung bluetoothd cannot hold up
/// serve for longer.
pub(super) const CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// The service data of the beacon: a fixed header, nine zero bytes and ten
/// random ones.
fn service_data() -> Vec<u8> {
    let mut data = vec![0xfc, 0x12, 0x8e, 0x01, 0x42];
    data.extend_from_slice(&[0; 9]);
    data.extend_from_slice(&rand::random::<[u8; 10]>());
    data
}

/// The advertisement object BlueZ reads when we register it.
struct Advertisement {
    service_data: Vec<u8>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    /// BlueZ dropped the advertisement (Bluetooth switched off, adapter gone).
    fn release(&self) {
        log::info!("quickshare: BlueZ released the BLE beacon");
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "broadcast".into()
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        vec![SERVICE_UUID.into()]
    }

    #[zbus(property, name = "ServiceData")]
    fn service_data(&self) -> HashMap<String, Value<'_>> {
        HashMap::from([(SERVICE_UUID.to_owned(), Value::from(self.service_data.clone()))])
    }
}

/// Our advertisement, on the air until dropped.
pub struct Beacon {
    /// Serves the advertisement object; BlueZ drops the advertisement when
    /// this connection closes.
    conn: Connection,
    adapter: OwnedObjectPath,
}

impl Drop for Beacon {
    fn drop(&mut self) {
        let path = ObjectPath::from_static_str_unchecked(ADV_PATH);
        if let Err(e) = self
            .conn
            .call_method(Some(BLUEZ), &self.adapter, Some(MANAGER), "UnregisterAdvertisement", &(path,))
        {
            log::debug!("quickshare: unregistering the BLE beacon: {e}");
        }
    }
}

/// A connection to the system bus whose calls time out, serving `adv`.
fn system_bus(adv: Option<Advertisement>) -> anyhow::Result<Connection> {
    let connect = || -> zbus::Result<Connection> {
        let mut builder = Builder::system()?.method_timeout(CALL_TIMEOUT);
        if let Some(adv) = adv {
            builder = builder.serve_at(ADV_PATH, adv)?;
        }
        builder.build()
    };
    connect().context("the system D-Bus is not available")
}

/// The first adapter (by path) that can advertise and is powered.
pub(super) fn adapter(conn: &Connection) -> anyhow::Result<OwnedObjectPath> {
    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
    let reply = conn
        .call_method(Some(BLUEZ), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
        .context("BlueZ is not available")?;
    let objects: Objects = reply.body().deserialize().context("reading BlueZ's objects")?;
    objects
        .into_iter()
        .filter(|(_, ifaces)| {
            let powered = ifaces.get(ADAPTER).and_then(|props| props.get("Powered")).and_then(|v| bool::try_from(v).ok());
            ifaces.contains_key(MANAGER) && powered == Some(true)
        })
        .map(|(path, _)| path)
        .min_by(|a, b| a.as_str().cmp(b.as_str()))
        .context("no powered Bluetooth adapter that can advertise")
}

/// Whether [`start`] can work: BlueZ is there with a powered adapter that
/// can advertise. Registers nothing.
pub fn probe() -> anyhow::Result<()> {
    adapter(&system_bus(None)?).map(drop)
}

/// Puts the beacon on the air, on the first powered adapter.
pub fn start() -> anyhow::Result<Beacon> {
    let adv = Advertisement { service_data: service_data() };
    let conn = system_bus(Some(adv))?;
    let adapter = adapter(&conn)?;
    let path = ObjectPath::from_static_str_unchecked(ADV_PATH);
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    conn.call_method(Some(BLUEZ), &adapter, Some(MANAGER), "RegisterAdvertisement", &(path, options))
        .context("registering the BLE advertisement")?;
    log::info!("quickshare: BLE beacon on {}", adapter.as_str());
    Ok(Beacon { conn, adapter })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_data_layout() {
        let data = service_data();
        assert_eq!(data.len(), 24);
        assert_eq!(data[..5], [0xfc, 0x12, 0x8e, 0x01, 0x42]);
        assert_eq!(data[5..14], [0; 9]);
        assert_ne!(data[14..], service_data()[14..], "the tail is random");
    }
}
