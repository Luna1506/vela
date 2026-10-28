//! The bandwidth upgrade of a transfer that started over Bluetooth: once it
//! is accepted, we offer the phone our Wi-Fi LAN address
//! (UPGRADE_PATH_AVAILABLE). The phone opens a TCP connection to serve's
//! port and introduces itself on it in plaintext (CLIENT_INTRODUCTION),
//! [`Rendezvous`] hands that connection to the waiting session, which acks
//! the introduction and moves its [`Conn`] over ([`Conn::switch_to`]): same
//! keys, same sequence numbers, no second UKEY2. If the phone does not come,
//! the transfer finishes over Bluetooth. Sequence from google/nearby
//! (Apache-2.0), `connections/implementation/bwu_manager.cc` and
//! `mediums/wifi_lan_bwu_handler.cc`.

use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, TcpStream};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::conn::Conn;
pub use super::proto::connections::BandwidthUpgradeNegotiationFrame;
use super::proto::connections::bandwidth_upgrade_negotiation_frame::upgrade_path_info::{Medium, WifiLanSocket};
use super::proto::connections::bandwidth_upgrade_negotiation_frame::{ClientIntroductionAck, EventType, UpgradePathInfo};
use super::proto::connections::{V1Frame, v1_frame};
use super::write_plain;

/// How long the phone has to connect over Wi-Fi after the offer.
#[cfg(not(test))]
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const ARRIVAL_TIMEOUT: Duration = Duration::from_millis(500);

/// `f` as a frame.
pub fn v1(f: BandwidthUpgradeNegotiationFrame) -> V1Frame {
    V1Frame {
        r#type: Some(v1_frame::FrameType::BandwidthUpgradeNegotiation as i32),
        bandwidth_upgrade_negotiation: Some(f),
        ..V1Frame::default()
    }
}

/// A frame with only its event set.
pub fn event(kind: EventType) -> BandwidthUpgradeNegotiationFrame {
    BandwidthUpgradeNegotiationFrame {
        event_type: Some(kind as i32),
        ..BandwidthUpgradeNegotiationFrame::default()
    }
}

/// The offer to move to Wi-Fi LAN at `ip`:`port`, with the ack after the
/// phone's introduction. Encryption stays on.
pub fn upgrade_path_available_wifi_lan(ip: Ipv4Addr, port: u16) -> BandwidthUpgradeNegotiationFrame {
    BandwidthUpgradeNegotiationFrame {
        upgrade_path_info: Some(UpgradePathInfo {
            medium: Some(Medium::WifiLan as i32),
            wifi_lan_socket: Some(WifiLanSocket {
                ip_address: Some(ip.octets().to_vec()),
                wifi_port: Some(i32::from(port)),
            }),
            supports_client_introduction_ack: Some(true),
            ..UpgradePathInfo::default()
        }),
        ..event(EventType::UpgradePathAvailable)
    }
}

/// Our plaintext answer to the phone's CLIENT_INTRODUCTION.
fn client_introduction_ack() -> V1Frame {
    v1(BandwidthUpgradeNegotiationFrame {
        client_introduction_ack: Some(ClientIntroductionAck {}),
        ..event(EventType::ClientIntroductionAck)
    })
}

/// The endpoint ids (`endpoint_id`, then `last_endpoint_id`) of a phone that
/// introduces itself on a new connection to move a transfer there.
pub fn client_introduction(v1: &V1Frame) -> Option<Vec<String>> {
    let f = v1.bandwidth_upgrade_negotiation.as_ref()?;
    if v1.r#type() != v1_frame::FrameType::BandwidthUpgradeNegotiation || f.event_type() != EventType::ClientIntroduction {
        return None;
    }
    let intro = f.client_introduction.as_ref()?;
    let ids: Vec<String> = [&intro.endpoint_id, &intro.last_endpoint_id].into_iter().flatten().cloned().collect();
    (!ids.is_empty()).then_some(ids)
}

/// Where sessions wait for the phone's new connection, by its endpoint id.
#[derive(Clone, Default)]
pub struct Rendezvous(Arc<Mutex<HashMap<String, mpsc::Sender<TcpStream>>>>);

/// A session waiting in a [`Rendezvous`]; dropping it stops waiting.
struct Expected {
    arrivals: mpsc::Receiver<TcpStream>,
    endpoint: String,
    rendezvous: Rendezvous,
}

impl Drop for Expected {
    fn drop(&mut self) {
        self.rendezvous.lock().remove(&self.endpoint);
    }
}

impl Rendezvous {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, mpsc::Sender<TcpStream>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn expect(&self, endpoint: &str) -> Expected {
        let (tx, arrivals) = mpsc::channel();
        self.lock().insert(endpoint.to_owned(), tx);
        Expected {
            arrivals,
            endpoint: endpoint.to_owned(),
            rendezvous: self.clone(),
        }
    }

    /// Acks the introduction of the phone with one of the endpoint ids
    /// `ids` on `stream` and hands the stream to the session waiting for it;
    /// false (and nothing written) if none waits.
    pub fn hand_over(&self, ids: &[String], stream: TcpStream) -> anyhow::Result<bool> {
        let waiting = ids.iter().find_map(|id| self.lock().get(id).cloned());
        let Some(waiting) = waiting else {
            return Ok(false);
        };
        write_plain(&stream, client_introduction_ack())?;
        Ok(waiting.send(stream).is_ok())
    }
}

/// What a Bluetooth session can offer: our address (`None`: no network),
/// serve's port, and where the phone's connection turns up.
#[derive(Clone)]
pub struct Offer {
    pub ip: Option<Ipv4Addr>,
    pub port: u16,
    pub rendezvous: Rendezvous,
}

/// An offered upgrade, until the phone came, said no or took too long.
pub struct Upgrade {
    waiting: Option<(Expected, Instant)>,
}

impl Upgrade {
    /// Offers phone `endpoint` to move `conn` to Wi-Fi LAN (after accepting
    /// its transfer). `None` when there is no address to offer.
    pub fn start(conn: &mut Conn, offer: &Offer, endpoint: &str) -> anyhow::Result<Option<Upgrade>> {
        let Some(ip) = offer.ip else {
            log::info!("quickshare: no network address to offer, the transfer stays on Bluetooth");
            return Ok(None);
        };
        let expected = offer.rendezvous.expect(endpoint);
        conn.send_upgrade(upgrade_path_available_wifi_lan(ip, offer.port))?;
        log::info!("quickshare: offering {endpoint} to go on over Wi-Fi at {ip}:{}", offer.port);
        Ok(Some(Upgrade {
            waiting: Some((expected, Instant::now() + ARRIVAL_TIMEOUT)),
        }))
    }

    /// Moves `conn` once the phone's connection is there; gives up after
    /// [`ARRIVAL_TIMEOUT`].
    pub fn poll(&mut self, conn: &mut Conn) -> anyhow::Result<()> {
        let Some((expected, deadline)) = &self.waiting else {
            return Ok(());
        };
        match expected.arrivals.try_recv() {
            Ok(stream) => {
                self.waiting = None;
                log::info!("quickshare: the transfer goes on over Wi-Fi");
                conn.switch_to(Box::new(stream))?;
            }
            Err(TryRecvError::Empty) if Instant::now() < *deadline => {}
            Err(_) => {
                self.waiting = None;
                log::info!("quickshare: the phone did not connect over Wi-Fi, the transfer stays on Bluetooth");
            }
        }
        Ok(())
    }

    /// A bandwidth upgrade frame from the phone.
    pub fn on_frame(&mut self, f: &BandwidthUpgradeNegotiationFrame) {
        if f.event_type() == EventType::UpgradeFailure && self.waiting.take().is_some() {
            log::info!("quickshare: the phone cannot go on over Wi-Fi, the transfer stays on Bluetooth");
        }
    }
}

/// Our IPv4 address for the phone: one of those mDNS announces (interfaces
/// that are up, not loopback or point-to-point), not link-local (the phone
/// refuses those), preferably on the interface of the default route.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let addrs: Vec<(String, Ipv4Addr)> = interfaces
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_p2p() && !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) => Some((i.name, a.ip)),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect();
    let routes = fs::read_to_string("/proc/net/route").unwrap_or_default();
    choose_ipv4(&addrs, &default_route_interfaces(&routes))
}

/// The interfaces with an IPv4 default route in `/proc/net/route`, lowest
/// metric first.
fn default_route_interfaces(table: &str) -> Vec<String> {
    const RTF_UP: u32 = 1;
    let mut routes: Vec<(u32, String)> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (name, dest, flags, metric, mask) = (f.first()?, f.get(1)?, f.get(3)?, f.get(6)?, f.get(7)?);
            let flags = u32::from_str_radix(flags, 16).ok()?;
            let default = *dest == "00000000" && *mask == "00000000" && flags & RTF_UP != 0;
            default.then(|| Some((metric.parse().ok()?, (*name).to_owned())))?
        })
        .collect();
    routes.sort();
    routes.into_iter().map(|(_, name)| name).collect()
}

fn choose_ipv4(addrs: &[(String, Ipv4Addr)], preferred: &[String]) -> Option<Ipv4Addr> {
    let usable: Vec<&(String, Ipv4Addr)> = addrs
        .iter()
        .filter(|(_, ip)| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified())
        .collect();
    preferred
        .iter()
        .find_map(|name| usable.iter().find(|(n, _)| n == name))
        .or_else(|| usable.first())
        .map(|(_, ip)| *ip)
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use crate::quickshare::conn::{Incoming, Link};
    use crate::quickshare::proto::connections::bandwidth_upgrade_negotiation_frame::ClientIntroduction;
    use crate::quickshare::{POLL, read_plain};
    use std::io::{self, Read, Write};
    use std::net::SocketAddrV4;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A link that counts what is written to it.
    pub(crate) struct Counted<L> {
        pub(crate) inner: L,
        pub(crate) written: Arc<AtomicUsize>,
    }

    impl<L: Link> Counted<L> {
        pub(crate) fn new(inner: L) -> (Counted<L>, Arc<AtomicUsize>) {
            let written = Arc::new(AtomicUsize::new(0));
            (
                Counted {
                    inner,
                    written: written.clone(),
                },
                written,
            )
        }
    }

    impl<L: Link> Read for Counted<L> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl<L: Link> Write for Counted<L> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let n = self.inner.write(buf)?;
            self.written.fetch_add(n, Ordering::SeqCst);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl<L: Link> Link for Counted<L> {
        fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
            self.inner.reader()
        }

        fn close(&self) -> io::Result<()> {
            self.inner.close()
        }
    }

    /// The phone's part, as the responder: waits for the offer, connects to
    /// it, introduces itself as `endpoint`, takes the ack and moves `conn`
    /// over. Returns the byte count of the new connection once moved.
    pub(crate) fn respond(conn: &mut Conn, endpoint: &str) -> Arc<AtomicUsize> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let offer = loop {
            assert!(Instant::now() < deadline, "no UPGRADE_PATH_AVAILABLE");
            if let Some(Incoming::Upgrade(f)) = conn.recv(POLL).unwrap()
                && f.event_type() == EventType::UpgradePathAvailable
            {
                break f;
            }
        };
        let info = offer.upgrade_path_info.unwrap();
        assert_eq!(info.medium(), Medium::WifiLan);
        assert!(info.supports_client_introduction_ack());
        let socket = info.wifi_lan_socket.unwrap();
        let ip: [u8; 4] = socket.ip_address.unwrap().try_into().unwrap();
        let addr = SocketAddrV4::new(ip.into(), u16::try_from(socket.wifi_port.unwrap()).unwrap());
        let tcp = TcpStream::connect(addr).unwrap();
        let introduction = BandwidthUpgradeNegotiationFrame {
            client_introduction: Some(ClientIntroduction {
                endpoint_id: Some(endpoint.into()),
                ..Default::default()
            }),
            ..event(EventType::ClientIntroduction)
        };
        write_plain(&tcp, v1(introduction)).unwrap();
        let ack = read_plain(&tcp).unwrap();
        assert_eq!(ack.bandwidth_upgrade_negotiation.unwrap().event_type(), EventType::ClientIntroductionAck);
        let (tcp, written) = Counted::new(tcp);
        conn.switch_to(Box::new(tcp)).unwrap();
        while conn.switching() {
            assert!(Instant::now() < deadline, "the switch did not end");
            assert_ne!(conn.recv(POLL).unwrap(), Some(Incoming::Disconnected));
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Counted, respond};
    use super::*;
    use crate::quickshare::conn::Incoming;
    use crate::quickshare::endpoint::{DeviceKind, EndpointInfo};
    use crate::quickshare::proto::sharing;
    use crate::quickshare::{Decision, Event, Outcome, POLL, Step, close, inbound, outbound, sharing_frame, weave};
    use prost::Message;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[test]
    fn upgrade_path_available_wifi_lan() {
        let f = super::upgrade_path_available_wifi_lan(Ipv4Addr::new(192, 168, 1, 23), 47392);
        assert_eq!(f.event_type(), EventType::UpgradePathAvailable);
        // event 1; path info: WIFI_LAN, socket 192.168.1.23:47392, ack.
        let expected = [
            0x08, 0x01, 0x12, 0x10, 0x08, 0x05, 0x1a, 0x0a, 0x0a, 0x04, 192, 168, 1, 23, 0x10, 0xa0, 0xf2, 0x02, 0x48, 0x01,
        ];
        assert_eq!(f.encode_to_vec(), expected);
        let frame = v1(f);
        assert_eq!(frame.r#type(), v1_frame::FrameType::BandwidthUpgradeNegotiation);
    }

    #[test]
    fn client_introduction_names_the_phone() {
        let mut f = event(EventType::ClientIntroduction);
        assert_eq!(client_introduction(&v1(f.clone())), None, "no introduction");
        f.client_introduction = Some(Default::default());
        assert_eq!(client_introduction(&v1(f.clone())), None, "no endpoint id");
        f.client_introduction.as_mut().unwrap().last_endpoint_id = Some("LAST".into());
        assert_eq!(client_introduction(&v1(f.clone())), Some(vec!["LAST".into()]));
        f.client_introduction.as_mut().unwrap().endpoint_id = Some("PHON".into());
        assert_eq!(client_introduction(&v1(f)), Some(vec!["PHON".into(), "LAST".into()]));
        assert_eq!(client_introduction(&v1(event(EventType::UpgradePathAvailable))), None);
        let ack = client_introduction_ack().bandwidth_upgrade_negotiation.unwrap();
        assert_eq!(ack.event_type(), EventType::ClientIntroductionAck);
    }

    #[test]
    fn the_address_comes_from_the_default_route() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                     eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                     wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n\
                     tun0\t00000000\t00000000\t0000\t0\t0\t50\t00000000\t0\t0\t0\n";
        assert_eq!(default_route_interfaces(table), ["eth0", "wlan0"], "by metric, routes that are up");
        let addrs = vec![
            ("docker0".to_owned(), Ipv4Addr::new(172, 17, 0, 1)),
            ("wlan0".to_owned(), Ipv4Addr::new(192, 168, 1, 23)),
            ("eth0".to_owned(), Ipv4Addr::new(169, 254, 3, 4)),
        ];
        let preferred = default_route_interfaces(table);
        assert_eq!(
            choose_ipv4(&addrs, &preferred),
            Some(Ipv4Addr::new(192, 168, 1, 23)),
            "eth0 only has link-local"
        );
        assert_eq!(choose_ipv4(&addrs, &[]), Some(Ipv4Addr::new(172, 17, 0, 1)), "no default route");
        assert_eq!(choose_ipv4(&addrs[2..], &preferred), None);
    }

    fn me() -> EndpointInfo {
        EndpointInfo {
            kind: DeviceKind::Phone,
            name: Some("Galaxy".into()),
        }
    }

    struct Session {
        events: mpsc::Receiver<Event>,
        decisions: mpsc::Sender<Decision>,
        thread: thread::JoinHandle<anyhow::Result<()>>,
    }

    /// `handle_link` on our end of a Bluetooth pair, offering 127.0.0.1 on
    /// a listener whose connections go through `handle_tcp` (serve's path).
    fn session(ours: weave::WeaveLink) -> Session {
        let rendezvous = Rendezvous::default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let r = rendezvous.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let (events, _) = mpsc::channel();
                let (_, decisions) = mpsc::channel();
                let r = r.clone();
                thread::spawn(move || inbound::handle_tcp(stream.unwrap(), 99, events, decisions, Arc::new(AtomicBool::new(false)), &r));
            }
        });
        let offer = Offer {
            ip: Some(Ipv4Addr::LOCALHOST),
            port,
            rendezvous,
        };
        let (tx, events) = mpsc::channel();
        let (decisions, rx) = mpsc::channel();
        let thread = thread::spawn(move || inbound::handle_link(ours, 1, tx, rx, Arc::new(AtomicBool::new(false)), Some(offer)));
        Session { events, decisions, thread }
    }

    fn finished(events: &mpsc::Receiver<Event>) -> (Outcome, Vec<std::path::PathBuf>) {
        loop {
            if let Event::Finished { outcome, saved, .. } = events.recv_timeout(Duration::from_secs(20)).unwrap() {
                return (outcome, saved);
            }
        }
    }

    /// The phone's handshake over Bluetooth, an introduction for one file of
    /// `data.len()` bytes and our accept.
    fn offer_and_accept(phone: impl crate::quickshare::conn::Link, s: &Session, data: &[u8], dest: &std::path::Path) -> Conn {
        let Step::Next(mut conn) = outbound::connect_link(phone, &me(), *b"PHON", &AtomicBool::new(false), |_| {}).unwrap() else {
            panic!("handshake ended early");
        };
        let intro = sharing::IntroductionFrame {
            file_metadata: vec![sharing::FileMetadata {
                name: Some("clip.bin".into()),
                payload_id: Some(5),
                size: Some(data.len() as i64),
                ..Default::default()
            }],
            ..Default::default()
        };
        conn.send_sharing(&sharing_frame(sharing::V1Frame {
            r#type: Some(sharing::v1_frame::FrameType::Introduction as i32),
            introduction: Some(intro),
            ..Default::default()
        }))
        .unwrap();
        loop {
            if let Event::Incoming(offer) = s.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                assert_eq!(offer.device, "Galaxy");
                break;
            }
        }
        s.decisions.send(Decision::Accept(dest.to_path_buf())).unwrap();
        assert_eq!(outbound::await_response(&mut conn, &AtomicBool::new(false)).unwrap(), Step::Next(()));
        conn
    }

    fn send_chunks(conn: &mut Conn, data: &[u8], from: usize, to: usize) {
        for start in (from..to).step_by(16 * 1024) {
            let end = (start + 16 * 1024).min(to);
            conn.send_file_chunk(5, data.len() as i64, start as i64, &data[start..end], false).unwrap();
        }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    #[test]
    fn upgrade_moves_session_to_tcp_keeping_counters() {
        let dest = tempfile::tempdir().unwrap();
        let data = data(300 * 1024);
        let (ours, phone) = weave::fake::pair(185);
        let (phone, over_ble) = Counted::new(phone);
        let s = session(ours);
        let mut conn = offer_and_accept(phone, &s, &data, dest.path());

        // Half over Bluetooth, then the upgrade, the rest over TCP. Every
        // frame on either link is encrypted with the one key pair and
        // sequence: a counter starting over would fail to decrypt.
        send_chunks(&mut conn, &data, 0, data.len() / 2);
        let ble_before = over_ble.load(Ordering::SeqCst);
        let over_tcp = respond(&mut conn, "PHON");
        let ble_switch = over_ble.load(Ordering::SeqCst) - ble_before;
        assert!(
            ble_switch < 4096,
            "only the switch frames (and keep-alives) went over Bluetooth meanwhile: {ble_switch}"
        );
        send_chunks(&mut conn, &data, data.len() / 2, data.len());
        conn.send_file_chunk(5, data.len() as i64, data.len() as i64, &[], true).unwrap();

        let (outcome, saved) = finished(&s.events);
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(std::fs::read(&saved[0]).unwrap(), data);
        assert!(over_tcp.load(Ordering::SeqCst) > data.len() / 2, "the second half went over TCP");
        assert_eq!(over_ble.load(Ordering::SeqCst) - ble_before, ble_switch, "and nothing more over Bluetooth");
        // Our DISCONNECTION comes over TCP.
        let deadline = Instant::now() + Duration::from_secs(5);
        while conn.recv(POLL).unwrap() != Some(Incoming::Disconnected) {
            assert!(Instant::now() < deadline, "no DISCONNECTION");
        }
        drop(conn);
        s.thread.join().unwrap().unwrap();
    }

    fn falls_back(say_no: bool) {
        let dest = tempfile::tempdir().unwrap();
        let data = data(100 * 1024);
        let (ours, phone) = weave::fake::pair(185);
        let s = session(ours);
        let mut conn = offer_and_accept(phone, &s, &data, dest.path());
        send_chunks(&mut conn, &data, 0, data.len() / 2);
        if say_no {
            conn.send_upgrade(event(EventType::UpgradeFailure)).unwrap();
        } else {
            // Past the time the phone had to connect.
            let until = Instant::now() + ARRIVAL_TIMEOUT * 2;
            while Instant::now() < until {
                conn.recv(POLL).unwrap();
            }
        }
        send_chunks(&mut conn, &data, data.len() / 2, data.len());
        conn.send_file_chunk(5, data.len() as i64, data.len() as i64, &[], true).unwrap();
        let (outcome, saved) = finished(&s.events);
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(std::fs::read(&saved[0]).unwrap(), data);
        close(&mut conn);
        s.thread.join().unwrap().unwrap();
    }

    #[test]
    fn without_wifi_the_transfer_finishes_over_bluetooth() {
        falls_back(false);
    }

    #[test]
    fn a_phone_that_cannot_switch_finishes_over_bluetooth() {
        falls_back(true);
    }

    #[test]
    fn only_a_waiting_session_gets_the_connection() {
        let rendezvous = Rendezvous::default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = || {
            let client = TcpStream::connect(addr).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            (client, listener.accept().unwrap().0)
        };
        let phone = ["PHON".to_owned()];
        let (mut client, server) = accept();
        assert!(!rendezvous.hand_over(&phone, server).unwrap());
        assert_eq!(std::io::Read::read(&mut client, &mut [0; 8]).unwrap(), 0, "closed without an ack");

        let expected = rendezvous.expect("PHON");
        let (client, server) = accept();
        assert!(rendezvous.hand_over(&["OTHR".to_owned(), "PHON".to_owned()], server).unwrap());
        let ack = crate::quickshare::read_plain(&client).unwrap();
        assert_eq!(ack.bandwidth_upgrade_negotiation.unwrap().event_type(), EventType::ClientIntroductionAck);
        assert!(expected.arrivals.try_recv().is_ok());

        drop(expected);
        let (_client, server) = accept();
        assert!(!rendezvous.hand_over(&phone, server).unwrap(), "no longer waiting");
    }
}
