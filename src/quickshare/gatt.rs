//! Being found and reached over Bluetooth LE, for phones that search over
//! Bluetooth only (Quick Share's "Share with Apple devices").
//!
//! [`Receiver::start`] registers with BlueZ over D-Bus (layout from
//! google/nearby, `connections/implementation/mediums/ble.cc` and its Apple
//! Weave peripheral):
//!
//! - a GATT application with one primary service `0xFEF3` holding the
//!   slot-0 characteristic (read: the medium advertisement, see [`blead`])
//!   and the two Weave characteristics: phones write packets to one, we
//!   send ours to them as indications of the other;
//! - an extended connectable advertisement with the medium advertisement as
//!   `0xFEF3` service data (left out when the adapter cannot send extended
//!   advertisements), and a legacy connectable one with the 17-byte header,
//!   which makes phones read the slot over GATT.
//!
//! All of it stays registered until the [`Receiver`] is dropped. What
//! phones do with the Weave characteristics arrives as [`Event`]s (for
//! [`weave::Hub`](super::weave::Hub)); [`Receiver::indicator`] sends
//! indications back.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, mpsc};

use anyhow::Context;
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::fdo::ObjectManager;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::ble::{self, BLUEZ, CALL_TIMEOUT};
use super::blead;
use super::endpoint::EndpointInfo;
use super::weave::Indicate;

const SERVICE_UUID: &str = "0000fef3-0000-1000-8000-00805f9b34fb";
/// Advertisement slot 0.
const SLOT_UUID: &str = "00000000-0000-3000-8000-000000000000";
/// Weave, phone to PC.
const WEAVE_IN_UUID: &str = "00000100-0004-1000-8000-001a11000101";
/// Weave, PC to phone.
const WEAVE_OUT_UUID: &str = "00000100-0004-1000-8000-001a11000102";

/// Our objects on our connection: the GATT application (an object manager
/// over the service and its characteristics) and the two advertisements.
const APP_PATH: &str = "/org/vela/quickshare/gatt";
const SERVICE_PATH: &str = "/org/vela/quickshare/gatt/service0";
const SLOT_PATH: &str = "/org/vela/quickshare/gatt/service0/slot0";
const WEAVE_IN_PATH: &str = "/org/vela/quickshare/gatt/service0/weave_in";
const WEAVE_OUT_PATH: &str = "/org/vela/quickshare/gatt/service0/weave_out";
const EXTENDED_ADV_PATH: &str = "/org/vela/quickshare/receive_adv0";
const LEGACY_ADV_PATH: &str = "/org/vela/quickshare/receive_adv1";

const GATT_MANAGER: &str = "org.bluez.GattManager1";
const CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";

/// What phones do with the Weave characteristics, in the order BlueZ tells
/// us. BlueZ sends indications to every subscribed phone and does not say
/// which one subscribed, so only writes name the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `device` wrote `data`, one Weave packet. `mtu` is the link's ATT MTU
    /// as BlueZ reports it (0 when it does not).
    Write { device: OwnedObjectPath, data: Vec<u8>, mtu: u16 },
    /// A phone turned indications on.
    Subscribed,
    /// The last subscriber turned indications off (or went away).
    Unsubscribed,
    /// A phone confirmed an indication.
    Confirmed,
    /// BlueZ dropped an advertisement (Bluetooth switched off, adapter
    /// gone): phones no longer find us.
    Released,
}

/// The errors BlueZ turns into ATT errors.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum BluezError {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidOffset(String),
    NotSupported(String),
    Failed(String),
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn service_path() -> ObjectPath<'static> {
    ObjectPath::from_static_str_unchecked(SERVICE_PATH)
}

struct Service;

#[zbus::interface(name = "org.bluez.GattService1")]
impl Service {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        SERVICE_UUID.into()
    }

    #[zbus(property)]
    fn primary(&self) -> bool {
        true
    }
}

/// Advertisement slot 0: the medium advertisement, read by phones that saw
/// the legacy header.
struct Slot {
    value: Vec<u8>,
}

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl Slot {
    /// The value from `offset` on (a long read takes several requests).
    fn read_value(&self, options: HashMap<String, OwnedValue>) -> Result<Vec<u8>, BluezError> {
        let offset = options.get("offset").and_then(|v| u16::try_from(v).ok()).unwrap_or(0) as usize;
        let value = self.value.get(offset..).map(<[u8]>::to_vec);
        value.ok_or_else(|| BluezError::InvalidOffset(format!("offset {offset} is past the end")))
    }

    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        SLOT_UUID.into()
    }

    #[zbus(property)]
    fn service(&self) -> ObjectPath<'static> {
        service_path()
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["read".into()]
    }
}

/// Weave packets from phones (write with response).
struct WeaveIn {
    events: mpsc::Sender<Event>,
}

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl WeaveIn {
    fn write_value(&self, value: Vec<u8>, options: HashMap<String, OwnedValue>) -> Result<(), BluezError> {
        let offset = options.get("offset").and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
        if offset != 0 {
            // A Weave packet always fits one write.
            return Err(BluezError::NotSupported(format!("write at offset {offset}")));
        }
        let device = options
            .get("device")
            .and_then(|v| <&ObjectPath>::try_from(&**v).ok())
            .map(|p| OwnedObjectPath::from(p.to_owned()))
            .ok_or_else(|| BluezError::Failed("a write without a device".into()))?;
        let mtu = options.get("mtu").and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
        log::debug!("quickshare: BLE write from {} (MTU {mtu}): {}", device.as_str(), hex(&value));
        let _ = self.events.send(Event::Write { device, data: value, mtu });
        Ok(())
    }

    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        WEAVE_IN_UUID.into()
    }

    #[zbus(property)]
    fn service(&self) -> ObjectPath<'static> {
        service_path()
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["write".into()]
    }
}

/// Weave packets to phones: each change of `Value` is one indication
/// ([`Receiver::indicator`]); BlueZ calls `Confirm` when the phone confirmed
/// it.
struct WeaveOut {
    events: mpsc::Sender<Event>,
}

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl WeaveOut {
    fn start_notify(&self) {
        log::debug!("quickshare: a phone subscribed to BLE indications");
        let _ = self.events.send(Event::Subscribed);
    }

    fn stop_notify(&self) {
        log::debug!("quickshare: BLE indications off");
        let _ = self.events.send(Event::Unsubscribed);
    }

    fn confirm(&self) {
        let _ = self.events.send(Event::Confirmed);
    }

    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        WEAVE_OUT_UUID.into()
    }

    #[zbus(property)]
    fn service(&self) -> ObjectPath<'static> {
        service_path()
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["indicate".into()]
    }

    #[zbus(property)]
    fn value(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn service_data(data: &[u8]) -> HashMap<String, Value<'static>> {
    HashMap::from([(SERVICE_UUID.to_owned(), Value::from(data.to_vec()))])
}

/// The medium advertisement as an extended connectable advertisement.
struct ExtendedAdvertisement {
    medium: Vec<u8>,
    events: mpsc::Sender<Event>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl ExtendedAdvertisement {
    /// BlueZ dropped the advertisement (Bluetooth switched off, adapter gone).
    fn release(&self) {
        log::info!("quickshare: BlueZ released the extended Bluetooth advertisement");
        let _ = self.events.send(Event::Released);
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property)]
    fn service_data(&self) -> HashMap<String, Value<'static>> {
        service_data(&self.medium)
    }

    #[zbus(property)]
    fn secondary_channel(&self) -> String {
        "1M".into()
    }
}

/// The header as a legacy connectable advertisement.
struct LegacyAdvertisement {
    header: [u8; 17],
    events: mpsc::Sender<Event>,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl LegacyAdvertisement {
    /// BlueZ dropped the advertisement (Bluetooth switched off, adapter gone).
    fn release(&self) {
        log::info!("quickshare: BlueZ released the Bluetooth advertisement");
        let _ = self.events.send(Event::Released);
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property)]
    fn service_data(&self) -> HashMap<String, Value<'static>> {
        service_data(&self.header)
    }
}

/// Our GATT service and advertisements, registered until dropped.
pub struct Receiver {
    /// Serves our objects; BlueZ also drops what we registered when this
    /// connection closes.
    conn: Connection,
    adapter: OwnedObjectPath,
    application: bool,
    /// The registered advertisements.
    advertisements: Vec<&'static str>,
}

impl Receiver {
    /// Makes endpoint `endpoint_id` with `info` visible and connectable
    /// over Bluetooth LE on the first powered adapter. Weave writes and
    /// subscriptions (and BlueZ releasing an advertisement) go to
    /// `incoming`. Fails if BlueZ, an adapter, the GATT
    /// service or the legacy advertisement is not there; an extended
    /// advertisement that fails is only logged.
    pub fn start(endpoint_id: [u8; 4], info: &EndpointInfo, incoming: mpsc::Sender<Event>) -> anyhow::Result<Receiver> {
        let medium = blead::medium_advertisement(endpoint_id, info);
        let events = incoming.clone();
        let connect = || -> zbus::Result<Connection> {
            Builder::system()?
                .method_timeout(CALL_TIMEOUT)
                .serve_at(SERVICE_PATH, Service)?
                .serve_at(SLOT_PATH, Slot { value: medium.clone() })?
                .serve_at(WEAVE_IN_PATH, WeaveIn { events: events.clone() })?
                .serve_at(WEAVE_OUT_PATH, WeaveOut { events: events.clone() })?
                .serve_at(APP_PATH, ObjectManager)?
                .serve_at(
                    EXTENDED_ADV_PATH,
                    ExtendedAdvertisement {
                        medium: medium.clone(),
                        events,
                    },
                )?
                .build()
        };
        let conn = connect().context("the system D-Bus is not available")?;
        let adapter = ble::adapter(&conn)?;
        let mut receiver = Receiver {
            conn,
            adapter,
            application: false,
            advertisements: Vec::new(),
        };

        let app = ObjectPath::from_static_str_unchecked(APP_PATH);
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        receiver
            .conn
            .call_method(Some(BLUEZ), &receiver.adapter, Some(GATT_MANAGER), "RegisterApplication", &(app, options))
            .context("registering the GATT service")?;
        receiver.application = true;

        let extended = match receiver.advertise(EXTENDED_ADV_PATH) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("quickshare: no extended Bluetooth advertisement, only the legacy one: {e:#}");
                let _ = receiver.conn.object_server().remove::<ExtendedAdvertisement, _>(EXTENDED_ADV_PATH);
                false
            }
        };
        let header = blead::header(&medium, extended);
        receiver
            .conn
            .object_server()
            .at(LEGACY_ADV_PATH, LegacyAdvertisement { header, events: incoming })?;
        receiver.advertise(LEGACY_ADV_PATH)?;
        log::info!(
            "quickshare: visible over Bluetooth on {} ({} advertisement(s))",
            receiver.adapter.as_str(),
            receiver.advertisements.len()
        );
        log::debug!("quickshare: BLE medium advertisement {}, header {}", hex(&medium), hex(&header));
        Ok(receiver)
    }

    /// Sends one packet to the phones that turned indications on, as a
    /// change of the Weave characteristic's `Value`.
    pub fn indicator(&self) -> Indicate {
        let conn = self.conn.clone();
        Arc::new(move |packet: &[u8]| {
            let changed = HashMap::from([("Value", Value::from(packet.to_vec()))]);
            let body = (CHARACTERISTIC, changed, Vec::<&str>::new());
            conn.emit_signal(
                None::<zbus::names::BusName<'_>>,
                WEAVE_OUT_PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &body,
            )
            .map_err(io::Error::other)
        })
    }

    /// Registers the advertisement at `path`.
    fn advertise(&mut self, path: &'static str) -> anyhow::Result<()> {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let adv = ObjectPath::from_static_str_unchecked(path);
        self.conn
            .call_method(Some(BLUEZ), &self.adapter, Some(ble::MANAGER), "RegisterAdvertisement", &(adv, options))
            .with_context(|| format!("registering the Bluetooth advertisement {path}"))?;
        self.advertisements.push(path);
        Ok(())
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        for path in self.advertisements.drain(..).rev() {
            let adv = ObjectPath::from_static_str_unchecked(path);
            if let Err(e) = self
                .conn
                .call_method(Some(BLUEZ), &self.adapter, Some(ble::MANAGER), "UnregisterAdvertisement", &(adv,))
            {
                log::debug!("quickshare: unregistering {path}: {e}");
            }
        }
        if self.application {
            let app = ObjectPath::from_static_str_unchecked(APP_PATH);
            if let Err(e) = self
                .conn
                .call_method(Some(BLUEZ), &self.adapter, Some(GATT_MANAGER), "UnregisterApplication", &(app,))
            {
                log::debug!("quickshare: unregistering the GATT service: {e}");
            }
        }
        // The objects hold senders of the event channel and an indicator
        // holds the connection: removing them ends the channel, so whoever
        // reads it (and the indicator with it) lets go.
        let server = self.conn.object_server();
        let _ = server.remove::<WeaveIn, _>(WEAVE_IN_PATH);
        let _ = server.remove::<WeaveOut, _>(WEAVE_OUT_PATH);
        let _ = server.remove::<ExtendedAdvertisement, _>(EXTENDED_ADV_PATH);
        let _ = server.remove::<LegacyAdvertisement, _>(LEGACY_ADV_PATH);
    }
}
