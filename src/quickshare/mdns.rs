//! Announcing this computer and finding others over mDNS.
//!
//! Quick Share devices announce `SERVICE_TYPE` with an instance name that
//! carries their endpoint id and a TXT record `n` with their endpoint info
//! (kind and name), base64.

use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};

use super::endpoint::{DeviceKind, EndpointInfo, SERVICE_TYPE, b64, instance_name, parse_instance_name, unb64};

/// A TXT string is at most 255 bytes: `n=` and the base64 of at most 189
/// bytes (1 + 16 + 1 + a name of 171 bytes).
const MAX_NAME: usize = 171;
/// How long dropping an [`Advertisement`] waits for the daemon to send the
/// goodbye.
const UNREGISTER_WAIT: Duration = Duration::from_secs(1);

/// This computer, announced until dropped.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        match self.daemon.unregister(&self.fullname) {
            Ok(done) => {
                let _ = done.recv_timeout(UNREGISTER_WAIT);
            }
            Err(e) => log::warn!("quickshare: cannot withdraw the mDNS announcement: {e}"),
        }
    }
}

/// Announces `info` as endpoint `id` on `port`, on every IPv4 address (also
/// ones that appear later).
pub fn advertise(daemon: &ServiceDaemon, id: [u8; 4], info: &EndpointInfo, port: u16) -> anyhow::Result<Advertisement> {
    let host = format!("vela-{}.local.", String::from_utf8_lossy(&id));
    let txt = [("n", txt_value(info))];
    let mut service = ServiceInfo::new(SERVICE_TYPE, &instance_name(&id), &host, (), port, &txt[..])
        .context("mDNS announcement")?
        .enable_addr_auto();
    service.set_interfaces(vec![IfKind::IPv4]);
    let fullname = service.get_fullname().to_owned();
    daemon.register(service).context("mDNS announcement")?;
    Ok(Advertisement {
        daemon: daemon.clone(),
        fullname,
    })
}

/// The TXT value `n`: the endpoint info with the name cut to fit.
fn txt_value(info: &EndpointInfo) -> String {
    let name = info.name.as_deref().map(|n| {
        let mut end = n.len().min(MAX_NAME);
        while !n.is_char_boundary(end) {
            end -= 1;
        }
        n[..end].to_owned()
    });
    b64(&EndpointInfo { kind: info.kind, name }.encode())
}

/// A Quick Share device nearby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its endpoint id (four letters or digits).
    pub endpoint: String,
    /// None when it hides its name (phones do so even with visibility
    /// "Everyone") or announces an empty one.
    pub name: Option<String>,
    pub kind: DeviceKind,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// Found, or its name or address changed.
    Up(Peer),
    /// Gone or without endpoint info, by endpoint id. May name a device never
    /// reported up.
    Down(String),
}

/// Looks for other devices until `daemon.stop_browse(SERVICE_TYPE)` (or the
/// receiver is dropped and the next event arrives). Our own announcement
/// (`own`) and entries whose endpoint info is missing or broken are left
/// out; devices that hide their name are listed without one. IPv4 addresses
/// on the network are preferred.
pub fn browse(daemon: &ServiceDaemon, own: [u8; 4]) -> anyhow::Result<mpsc::Receiver<PeerEvent>> {
    let events = daemon.browse(SERVICE_TYPE).context("mDNS search")?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("quickshare-browse".into()).spawn(move || {
        while let Ok(event) = events.recv() {
            let event = match event {
                ServiceEvent::ServiceResolved(s) => {
                    let addrs: Vec<SocketAddr> = s
                        .get_addresses()
                        .iter()
                        .map(|ip| match ip {
                            mdns_sd::ScopedIp::V6(v6) => std::net::SocketAddrV6::new(*v6.addr(), s.get_port(), 0, v6.scope_id().index).into(),
                            ip => SocketAddr::new(ip.to_ip_addr(), s.get_port()),
                        })
                        .collect();
                    resolved(s.get_fullname(), s.get_property_val_str("n"), &addrs, own)
                }
                ServiceEvent::ServiceRemoved(_, fullname) => removed(&fullname, own),
                ServiceEvent::SearchStopped(_) => break,
                _ => None,
            };
            if let Some(event) = event
                && tx.send(event).is_err()
            {
                break;
            }
        }
    })?;
    Ok(rx)
}

/// The endpoint id in a service's full name, unless it is `own`.
fn endpoint_of(fullname: &str, own: [u8; 4]) -> Option<String> {
    let instance = fullname.strip_suffix(SERVICE_TYPE)?.strip_suffix('.')?;
    let id = parse_instance_name(instance).filter(|id| *id != own)?;
    Some(String::from_utf8_lossy(&id).into_owned())
}

fn resolved(fullname: &str, txt: Option<&str>, addrs: &[SocketAddr], own: [u8; 4]) -> Option<PeerEvent> {
    let endpoint = endpoint_of(fullname, own)?;
    let Some(info) = txt.and_then(unb64).and_then(|b| EndpointInfo::decode(&b)) else {
        return Some(PeerEvent::Down(endpoint));
    };
    let (kind, name) = (info.kind, info.name.filter(|n| !n.trim().is_empty()));
    let addr = addrs.iter().copied().min_by_key(|a| (rank(a), *a))?;
    Some(PeerEvent::Up(Peer { endpoint, name, kind, addr }))
}

/// Lower is better: IPv4 on the network, then loopback and link-local
/// IPv4, then IPv6.
fn rank(addr: &SocketAddr) -> u8 {
    match addr {
        SocketAddr::V4(a) if a.ip().is_loopback() || a.ip().is_link_local() => 1,
        SocketAddr::V4(_) => 0,
        SocketAddr::V6(_) => 2,
    }
}

fn removed(fullname: &str, own: [u8; 4]) -> Option<PeerEvent> {
    endpoint_of(fullname, own).map(PeerEvent::Down)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fullname(id: &[u8; 4]) -> String {
        format!("{}.{SERVICE_TYPE}", instance_name(id))
    }

    fn txt(kind: DeviceKind, name: Option<&str>) -> String {
        b64(&EndpointInfo {
            kind,
            name: name.map(str::to_owned),
        }
        .encode())
    }

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    #[test]
    fn txt_value_fits_one_txt_string() {
        let long = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("ä".repeat(200)),
        };
        let v = txt_value(&long);
        assert!("n=".len() + v.len() <= 255, "{} bytes", v.len());
        let back = EndpointInfo::decode(&unb64(&v).unwrap()).unwrap();
        let name = back.name.unwrap();
        assert!(!name.is_empty() && "ä".repeat(200).starts_with(&name));
        let short = EndpointInfo {
            kind: DeviceKind::Laptop,
            name: Some("luna-laptop".into()),
        };
        assert_eq!(EndpointInfo::decode(&unb64(&txt_value(&short)).unwrap()).unwrap(), short);
    }

    #[test]
    fn a_resolved_phone_is_a_peer_reached_over_ipv4() {
        let got = resolved(
            &fullname(b"AB12"),
            Some(&txt(DeviceKind::Phone, Some("Galaxy S24"))),
            &addrs(&["[fe80::1%2]:4000", "127.0.0.1:4000", "169.254.3.3:4000", "192.168.1.20:4000"]),
            *b"OWN1",
        );
        assert_eq!(
            got,
            Some(PeerEvent::Up(Peer {
                endpoint: "AB12".into(),
                name: Some("Galaxy S24".into()),
                kind: DeviceKind::Phone,
                addr: "192.168.1.20:4000".parse().unwrap(),
            }))
        );
        let v6 = resolved(
            &fullname(b"AB12"),
            Some(&txt(DeviceKind::Phone, Some("P"))),
            &addrs(&["[fe80::1%2]:4000"]),
            *b"OWN1",
        );
        assert!(matches!(v6, Some(PeerEvent::Up(p)) if p.addr.is_ipv6()), "IPv6 if that is all there is");
    }

    /// What a Galaxy S25 Ultra announced with visibility "Everyone": the
    /// hidden bit set and no name.
    #[test]
    fn hidden_phone_is_listed_without_name() {
        let got = resolved(
            &format!("I1ZDUkX8n14AAA.{SERVICE_TYPE}"),
            Some("MtcZPx86EttsZrogwXf7V8Y"),
            &addrs(&["192.168.0.51:53601"]),
            *b"OWN1",
        );
        assert_eq!(
            got,
            Some(PeerEvent::Up(Peer {
                endpoint: "VCRE".into(),
                name: None,
                kind: DeviceKind::Phone,
                addr: "192.168.0.51:53601".parse().unwrap(),
            }))
        );
    }

    #[test]
    fn own_entries_and_entries_without_endpoint_info_are_ignored() {
        let phone = txt(DeviceKind::Phone, Some("Galaxy"));
        let a = addrs(&["192.168.1.20:4000"]);
        assert_eq!(resolved(&fullname(b"OWN1"), Some(&phone), &a, *b"OWN1"), None);
        // Hidden or an empty name: listed without a name.
        let (hidden, empty) = (txt(DeviceKind::Phone, None), txt(DeviceKind::Phone, Some(" ")));
        for t in [hidden, empty] {
            let got = resolved(&fullname(b"AB12"), Some(&t), &a, *b"OWN1");
            assert!(matches!(&got, Some(PeerEvent::Up(p)) if p.name.is_none() && p.endpoint == "AB12"), "{got:?}");
        }
        // No TXT, broken TXT: gone from the list if it was there.
        for t in [None, Some("!!")] {
            assert_eq!(resolved(&fullname(b"AB12"), t, &a, *b"OWN1"), Some(PeerEvent::Down("AB12".into())));
        }
        assert_eq!(resolved(&fullname(b"AB12"), Some(&phone), &[], *b"OWN1"), None, "no address");
        assert_eq!(
            resolved(&format!("printer.{SERVICE_TYPE}"), Some(&phone), &a, *b"OWN1"),
            None,
            "not a Quick Share name"
        );
    }

    #[test]
    fn a_removed_entry_is_down() {
        assert_eq!(removed(&fullname(b"AB12"), *b"OWN1"), Some(PeerEvent::Down("AB12".into())));
        assert_eq!(removed(&fullname(b"OWN1"), *b"OWN1"), None);
        assert_eq!(removed("junk", *b"OWN1"), None);
    }
}
