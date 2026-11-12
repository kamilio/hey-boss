//! Retained at first enrollment. Keychain grants access to this exact executable.
use std::{
    env,
    ffi::c_void,
    io::{self, IsTerminal, Read, Write},
    ptr,
};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainSetUserInteractionAllowed(allowed: u8) -> i32;
    fn SecKeychainFindGenericPassword(
        keychains: *const c_void,
        service_len: u32,
        service: *const u8,
        account_len: u32,
        account: *const u8,
        size: *mut u32,
        data: *mut *mut u8,
        item: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainAddGenericPassword(
        keychain: *const c_void,
        service_len: u32,
        service: *const u8,
        account_len: u32,
        account: *const u8,
        size: u32,
        data: *const u8,
        item: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainItemFreeContent(attrs: *mut c_void, data: *mut u8) -> i32;
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let (op, service, account) = match args.as_slice() {
        [op, account] if op == "read" || op == "create" => {
            (op.as_str(), "hey-boss.secrets", account)
        }
        [op, service, account] if op == "import" && service != "hey-boss.secrets" => {
            ("read", service.as_str(), account)
        }
        _ => std::process::exit(2),
    };
    if io::stdout().is_terminal() || (op == "create" && io::stdin().is_terminal()) {
        std::process::exit(2);
    }
    let mut out = io::stdout().lock();
    // Never unlock a keychain or ask the user to grant this process access.
    let status = unsafe { SecKeychainSetUserInteractionAllowed(0) };
    if status != 0 {
        return out.write_all(&status.to_be_bytes());
    }
    match op {
        "read" => {
            let mut size = 0;
            let mut data = ptr::null_mut();
            let status = unsafe {
                SecKeychainFindGenericPassword(
                    ptr::null(),
                    service.len() as u32,
                    service.as_ptr(),
                    account.len() as u32,
                    account.as_ptr(),
                    &mut size,
                    &mut data,
                    ptr::null_mut(),
                )
            };
            let reply = out.write_all(&status.to_be_bytes()).and_then(|()| {
                if status == 0 && size > 0 {
                    // Success returns a Security-owned allocation of exactly size bytes.
                    out.write_all(unsafe { std::slice::from_raw_parts(data, size as usize) })?;
                }
                Ok(())
            });
            if !data.is_null() {
                unsafe {
                    SecKeychainItemFreeContent(ptr::null_mut(), data);
                }
            }
            reply
        }
        "create" => {
            let mut bytes = Vec::new();
            io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 1024 * 1024 {
                std::process::exit(2);
            }
            // Add only. Duplicate creation must never update a machine key.
            let status = unsafe {
                SecKeychainAddGenericPassword(
                    ptr::null(),
                    service.len() as u32,
                    service.as_ptr(),
                    account.len() as u32,
                    account.as_ptr(),
                    bytes.len() as u32,
                    bytes.as_ptr(),
                    ptr::null_mut(),
                )
            };
            out.write_all(&status.to_be_bytes())
        }
        _ => std::process::exit(2),
    }
}
