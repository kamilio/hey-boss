//! Explicit Wi-Fi access. Unknown/redacted SSIDs fail closed.
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub(super) struct Network {
    pub ssid: String,
    pub address: Ipv4Addr,
    pub mask: Ipv4Addr,
}
impl Network {
    pub fn permits(&self, allow: &[String], peer: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask);
        allow.contains(&self.ssid)
            && self.address.is_private()
            && peer.is_private()
            && mask.count_ones() >= 8
            && mask.leading_ones() == mask.count_ones()
            && (u32::from(self.address) & mask) == (u32::from(peer) & mask)
            && (u32::from(peer) & !mask) != 0
            && (u32::from(peer) & !mask) != !mask
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn current() -> Option<Network> {
    None
}

#[cfg(target_os = "macos")]
pub(super) fn current() -> Option<Network> {
    use std::ffi::{CStr, c_char, c_void};
    type Id = *mut c_void;
    #[link(name = "CoreWLAN", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> Id;
        fn objc_autoreleasePoolPop(pool: Id);
    }
    unsafe fn send(object: Id, selector: &CStr) -> Id {
        if object.is_null() {
            return std::ptr::null_mut();
        }
        let call: unsafe extern "C" fn(Id, Id) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { call(object, sel_registerName(selector.as_ptr())) }
    }
    unsafe fn string(object: Id) -> Option<String> {
        let bytes = unsafe { send(object, c"UTF8String") };
        if bytes.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(bytes.cast()) }
            .to_str()
            .ok()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
    // CoreWLAN reads association state, without scanning or shell subprocesses.
    let pool = unsafe { objc_autoreleasePoolPush() };
    let names = unsafe {
        let client = send(objc_getClass(c"CWWiFiClient".as_ptr()), c"sharedWiFiClient");
        let interface = send(client, c"interface");
        string(send(interface, c"ssid")).zip(string(send(interface, c"interfaceName")))
    };
    unsafe { objc_autoreleasePoolPop(pool) };
    let (ssid, interface) = names?;
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
            && item.ifa_flags & (libc::IFF_UP as u32) != 0
            && unsafe { CStr::from_ptr(item.ifa_name) }.to_bytes() == interface.as_bytes()
        {
            let address = unsafe { &*item.ifa_addr.cast::<libc::sockaddr_in>() };
            let mask = unsafe { &*item.ifa_netmask.cast::<libc::sockaddr_in>() };
            found = Some(Network {
                ssid,
                address: Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
                mask: Ipv4Addr::from(mask.sin_addr.s_addr.to_ne_bytes()),
            });
            break;
        }
        cursor = item.ifa_next;
    }
    unsafe { libc::freeifaddrs(addresses) };
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_probe_handles_absent_or_redacted_wifi() {
        if let Some(network) = current() {
            assert!(!network.ssid.is_empty());
            assert!(!network.address.is_unspecified());
        }
    }

    #[test]

    fn only_named_wifi_and_direct_private_subnet_peers_are_allowed() {
        let mut network = Network {
            ssid: "Home".into(),
            address: "192.168.1.10".parse().unwrap(),
            mask: "255.255.255.0".parse().unwrap(),
        };
        let allowed = vec!["Home".into(), "Office".into()];
        assert!(network.permits(&allowed, "192.168.1.20".parse().unwrap()));
        for peer in [
            "192.168.2.20",
            "10.0.0.2",
            "8.8.8.8",
            "127.0.0.1",
            "192.168.1.0",
            "192.168.1.255",
        ] {
            assert!(!network.permits(&allowed, peer.parse().unwrap()), "{peer}");
        }
        assert!(!network.permits(&[], "192.168.1.20".parse().unwrap()));
        network.ssid = "Coffee shop".into();
        assert!(!network.permits(&allowed, "192.168.1.20".parse().unwrap()));
        network.ssid = "Home".into();
        network.mask = Ipv4Addr::UNSPECIFIED;
        assert!(!network.permits(&allowed, "192.168.1.20".parse().unwrap()));
        network.mask = "255.0.255.0".parse().unwrap();
        assert!(!network.permits(&allowed, "192.168.1.20".parse().unwrap()));
        network.address = "8.8.8.1".parse().unwrap();
        network.mask = "255.255.255.0".parse().unwrap();
        assert!(!network.permits(&allowed, "8.8.8.2".parse().unwrap()));
    }
}
