use chrono::{Local, TimeZone};

/// "0 bytes", "912 bytes", "4.2 KB", "1.5 GB" (1024-based, Windows labels).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} byte{}", if bytes == 1 { "" } else { "s" });
    }
    let mut v = bytes as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

/// Local date and time, or empty when unknown.
pub fn date(unix: i64) -> String {
    if unix == 0 {
        return String::new();
    }
    match Local.timestamp_opt(unix, 0).single() {
        Some(t) => t.format("%d-%m-%Y %H:%M").to_string(),
        None => String::new(),
    }
}

/// "12 MB/s"
pub fn speed(bytes_per_sec: f64) -> String {
    format!("{}/s", size(bytes_per_sec.max(0.0) as u64))
}

/// "About 3 minutes", "About 12 seconds".
pub fn eta(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return String::new();
    }
    let s = seconds.round() as u64;
    match s {
        0..=59 => format!("About {s} second{}", if s == 1 { "" } else { "s" }),
        60..=3599 => {
            let m = s.div_ceil(60);
            format!("About {m} minute{}", if m == 1 { "" } else { "s" })
        }
        _ => format!("About {:.1} hours", s as f64 / 3600.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(50 * 1024 * 1024), "50 MB");
    }
}
