//! Cached home public-IP verification; no HTTP lookup on the request path.
#[cfg(test)]
mod tests {
    use super::*;
    fn network() -> Network {
        Network {
            address: "192.168.1.10".parse().unwrap(),
            mask: "255.255.255.0".parse().unwrap(),
            interface: "en0".into(),
        }
    }
    #[test]
    fn verification_expires_and_is_bound_to_the_local_network() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(state.network(&network(), now).is_none());
        state.record(network(), true, now);
        assert!(state.network(&network(), now).is_some());
        assert!(state.network(&network(), now + MAX_AGE).is_none());
        let mut changed = network();
        changed.address = "192.168.2.10".parse().unwrap();
        assert!(state.network(&changed, now).is_none());
        state.record(network(), false, now);
        assert!(state.network(&network(), now).is_none());
    }
    #[test]
    fn only_direct_private_subnet_peers_are_allowed() {
        assert!(network().permits("192.168.1.20".parse().unwrap()));
        for peer in [
            "192.168.2.20",
            "8.8.8.8",
            "127.0.0.1",
            "192.168.1.0",
            "192.168.1.255",
        ] {
            assert!(!network().permits(peer.parse().unwrap()), "{peer}");
        }
        let mut invalid = network();
        invalid.mask = "255.0.255.0".parse().unwrap();
        assert!(!invalid.permits("192.168.1.20".parse().unwrap()));
    }
    #[test]
    fn public_ip_response_must_be_a_single_public_ipv4() {
        assert_eq!(
            parse_public_ip("8.8.8.8\n"),
            Some("8.8.8.8".parse().unwrap())
        );
        for bad in [
            "",
            "192.168.1.1",
            "127.0.0.1",
            "0.0.0.0",
            "169.254.1.1",
            "224.0.0.1",
            "100.64.0.1",
            "::1",
            "8.8.8.8 extra",
        ] {
            assert!(parse_public_ip(bad).is_none(), "{bad}");
        }
    }
}
use std::io::Read;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(30);
const MAX_AGE: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Network {
    pub address: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub interface: String,
}
impl Network {
    pub fn permits(&self, peer: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask);
        self.address.is_private()
            && peer.is_private()
            && mask.count_ones() >= 8
            && mask.leading_ones() == mask.count_ones()
            && (u32::from(self.address) & mask) == (u32::from(peer) & mask)
            && (u32::from(peer) & !mask) != 0
            && (u32::from(peer) & !mask) != !mask
    }
}

#[derive(Default)]
struct State {
    verified: Option<(Network, Instant)>,
}
impl State {
    fn record(&mut self, network: Network, matched: bool, now: Instant) {
        self.verified = matched.then_some((network, now));
    }
    fn network(&self, current: &Network, now: Instant) -> Option<Network> {
        let (network, checked) = self.verified.as_ref()?;
        (network == current && now.duration_since(*checked) < MAX_AGE).then(|| network.clone())
    }
}

#[derive(Clone, Default)]
pub(super) struct Guard {
    state: Arc<Mutex<State>>,
}
impl Guard {
    pub fn start(expected: Option<Ipv4Addr>) -> Self {
        let guard = Self::default();
        let Some(expected) = expected else {
            return guard;
        };
        let weak = Arc::downgrade(&guard.state);
        std::thread::spawn(move || {
            let mut previous = None;
            let mut next = Instant::now();
            while let Some(state) = weak.upgrade() {
                let network = current();
                let changed = network != previous;
                if changed {
                    state.lock().unwrap().verified = None;
                    previous = network.clone();
                }
                if changed || Instant::now() >= next {
                    // Only this background thread does HTTPS. No proxy, redirects, or
                    // unlimited response bodies; bind the probe to the checked interface.
                    let matched = network
                        .as_ref()
                        .is_some_and(|network| lookup(network.address) == Some(expected));
                    let after = current();
                    let mut state = state.lock().unwrap();
                    state.verified = None;
                    if let Some(network) = network.filter(|n| Some(n) == after.as_ref()) {
                        state.record(network, matched, Instant::now());
                    }
                    next = Instant::now() + REFRESH;
                }
                drop(state);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        guard
    }
    pub fn network(&self) -> Option<Network> {
        // Kernel-only route/interface reads catch local network changes immediately,
        // even between background probes. No DNS, subprocess, or packet is sent.
        let network = current()?;
        self.state.lock().ok()?.network(&network, Instant::now())
    }
}

pub(super) fn parse_public_ip(body: &str) -> Option<Ipv4Addr> {
    let ip: Ipv4Addr = body.trim().parse().ok()?;
    let [a, b, _, _] = ip.octets();
    (!ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 240
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 198 && (b == 18 || b == 19)))
        .then_some(ip)
}
fn lookup(address: Ipv4Addr) -> Option<Ipv4Addr> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .local_address(std::net::IpAddr::V4(address))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let response = client
        .get("https://api.ipify.org")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut body = String::new();
    response.take(65).read_to_string(&mut body).ok()?;
    (body.len() <= 64).then(|| parse_public_ip(&body)).flatten()
}

pub(super) fn current() -> Option<Network> {
    // UDP connect selects the route without sending any packets.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(1, 1, 1, 1), 443)).ok()?;
    let std::net::IpAddr::V4(address) = socket.local_addr().ok()?.ip() else {
        return None;
    };
    let mut addresses = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut addresses) } != 0 {
        return None;
    }
    let mut cursor = addresses;
    let mut found = None;
    while !cursor.is_null() {
        let item = unsafe { &*cursor };
        if !item.ifa_addr.is_null()
            && !item.ifa_netmask.is_null()
            && unsafe { (*item.ifa_addr).sa_family as i32 } == libc::AF_INET
            && item.ifa_flags & libc::IFF_UP as u32 != 0
            && item.ifa_flags & libc::IFF_LOOPBACK as u32 == 0
        {
            let addr = unsafe { &*item.ifa_addr.cast::<libc::sockaddr_in>() };
            if Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()) == address {
                let mask = unsafe { &*item.ifa_netmask.cast::<libc::sockaddr_in>() };
                found = Some(Network {
                    address,
                    mask: Ipv4Addr::from(mask.sin_addr.s_addr.to_ne_bytes()),
                    interface: unsafe { std::ffi::CStr::from_ptr(item.ifa_name) }
                        .to_string_lossy()
                        .into_owned(),
                });
                break;
            }
        }
        cursor = item.ifa_next;
    }
    unsafe { libc::freeifaddrs(addresses) };
    found.filter(|n| n.permits(n.address))
}
