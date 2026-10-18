//! 时间：ISO 8601 → Unix 秒，Unix 秒 → 本地日期（天号 = 1970-01-01 起的天数）。
//! 本地时区按系统设置，夏令时按各自日期换算（SystemTimeToTzSpecificLocalTime 用当年的规则）。

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

/// 1601 到 1970 的 100ns 数
const EPOCH_DIFF: i64 = 116_444_736_000_000_000;

pub fn now() -> i64 {
    let ft = unsafe { GetSystemTimeAsFileTime() };
    (((ft.dwHighDateTime as i64) << 32 | ft.dwLowDateTime as i64) - EPOCH_DIFF) / 10_000_000
}

/// 公历日期 → 天号（Howard Hinnant 的 days_from_civil）
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i32;
    era * 146097 + doe - 719468
}

/// 天号 → (年, 月, 日)
pub fn civil(day: i32) -> (i32, u32, u32) {
    let z = day + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i32, m, d)
}

/// 星期几，0 = 周日
pub fn weekday(day: i32) -> u32 {
    (day + 4).rem_euclid(7) as u32
}

/// "2026-10-17T17:08:14.600Z" / "...+00:00" → Unix 秒
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let n = |a: usize, l: usize| -> Option<i64> { s.get(a..a + l)?.parse().ok() };
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let day = days_from_civil(n(0, 4)? as i32, n(5, 2)? as u32, n(8, 2)? as u32) as i64;
    let mut t = day * 86400 + n(11, 2)? * 3600 + n(14, 2)? * 60 + n(17, 2)?;
    // 跳过小数秒，看时区
    let mut i = 19;
    while i < b.len() && (b[i] == b'.' || b[i].is_ascii_digit()) {
        i += 1;
    }
    if let Some(&sign) = b.get(i).filter(|c| **c == b'+' || **c == b'-') {
        let off = n(i + 1, 2)? * 3600 + n(i + 4, 2)? * 60;
        t -= if sign == b'+' { off } else { -off };
    }
    Some(t)
}

/// Unix 秒 → 本地天号
pub fn local_day(t: i64) -> i32 {
    let v = t * 10_000_000 + EPOCH_DIFF;
    let ft = FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 };
    let (mut utc, mut loc) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc).is_err() {
            return t.div_euclid(86400) as i32;
        }
    }
    days_from_civil(loc.wYear as i32, loc.wMonth as u32, loc.wDay as u32)
}

pub fn today() -> i32 {
    local_day(now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 18), 20744);
        for d in [-1000, 0, 59, 20744, 30000] {
            let (y, m, dd) = civil(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        // 2026-10-18 是周日
        assert_eq!(weekday(20744), 0);
    }

    #[test]
    fn iso() {
        let z = parse_iso("2026-10-17T17:08:14.600Z").unwrap();
        assert_eq!(z, 20743 * 86400 + 17 * 3600 + 8 * 60 + 14);
        assert_eq!(parse_iso("2026-10-17T17:08:14.600000+00:00"), Some(z));
        assert_eq!(parse_iso("2026-10-17T19:08:14+02:00"), Some(z));
        assert_eq!(parse_iso("bad"), None);
    }
}
