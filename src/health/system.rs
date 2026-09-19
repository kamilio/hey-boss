use super::Metrics;
#[cfg(target_os = "macos")]
use super::text;
#[cfg(target_os = "macos")]
use std::process::Command;

#[allow(clippy::unnecessary_cast)] // statvfs widths differ between Darwin and Linux.
pub(super) fn metrics() -> Metrics {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let mut result = Metrics {
        disk_path: home.clone(),
        memory_pressure: "Unavailable".into(),
        ..Metrics::default()
    };
    if let Ok(path) = std::ffi::CString::new(home) {
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } == 0 {
            let s = unsafe { info.assume_init() };
            result.disk_total_bytes = Some((s.f_blocks as u64).saturating_mul(s.f_frsize as u64));
            result.disk_available_bytes =
                Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64));
        }
    }
    memory(&mut result);
    result
}

#[cfg(target_os = "macos")]
fn memory(m: &mut Metrics) {
    m.memory_total_bytes = text(Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]))
        .ok()
        .and_then(|s| s.trim().parse().ok());
    if let Ok(stat) = text(&mut Command::new("/usr/bin/vm_stat")) {
        let page = stat
            .lines()
            .next()
            .and_then(|s| s.split("page size of ").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok());
        let value = |key: &str| {
            stat.lines()
                .find_map(|s| s.strip_prefix(key))
                .and_then(|s| s.trim().trim_end_matches('.').parse::<u64>().ok())
        };
        if let (Some(page), Some(free), Some(inactive)) =
            (page, value("Pages free:"), value("Pages inactive:"))
        {
            // Available is an estimate: free + inactive pages, not the complement of RSS.
            m.memory_available_bytes = Some((free + inactive).saturating_mul(page));
        }
    }
    if let Ok(s) =
        text(Command::new("/usr/sbin/sysctl").args(["-n", "kern.memorystatus_vm_pressure_level"]))
    {
        m.memory_pressure = match s.trim() {
            "1" => "Normal",
            "2" => "Warning",
            "4" => "Critical",
            _ => "Unavailable",
        }
        .into();
    }
    if let Ok(s) = text(Command::new("/usr/sbin/sysctl").args(["-n", "vm.swapusage"])) {
        m.swap_used_bytes = s
            .split("used = ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(parse_size);
    }
}
#[cfg(target_os = "macos")]
fn parse_size(s: &str) -> Option<u64> {
    let factor = match s.chars().last()? {
        'K' => 1024.0,
        'M' => 1048576.0,
        'G' => 1073741824.0,
        _ => 1.0,
    };
    Some((s.trim_end_matches(['K', 'M', 'G']).parse::<f64>().ok()? * factor) as u64)
}

#[cfg(not(target_os = "macos"))]
fn memory(m: &mut Metrics) {
    let Ok(contents) = std::fs::read_to_string("/proc/meminfo") else {
        return;
    };
    let value = |key: &str| {
        contents
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v * 1024)
    };
    m.memory_total_bytes = value("MemTotal:");
    m.memory_available_bytes = value("MemAvailable:");
    m.swap_used_bytes = value("SwapTotal:")
        .zip(value("SwapFree:"))
        .map(|(t, f)| t.saturating_sub(f));
    if let Some((total, available)) = m.memory_total_bytes.zip(m.memory_available_bytes) {
        m.memory_pressure = if available < total / 20 {
            "Critical"
        } else if available < total / 10 {
            "Warning"
        } else {
            "Normal"
        }
        .into();
    }
}
