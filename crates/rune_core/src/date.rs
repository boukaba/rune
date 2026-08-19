use crate::gc::{GcHeader, SemiSpace, TAG_DATE};
use std::sync::atomic::Ordering;

pub const DATE_SIZE: usize = 32;

/// Heap-allocated Date object.
/// Layout: [GcHeader(8) | tv:f64(8) | pad(8) | prototype(8)] = 32 bytes
/// `tv` is the time value: a finite integral Number of milliseconds since the
/// epoch, or NaN for an invalid date.
#[repr(C)]
pub struct RuneDate {
    header: GcHeader,
    tv: f64,
    _pad: u64,
    prototype: *mut u8,
}

impl RuneDate {
    pub fn allocate(gc: &mut SemiSpace, prototype: *mut u8) -> *mut u8 {
        let ptr = gc.alloc(DATE_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_DATE, Ordering::Relaxed);
            let d = ptr as *mut RuneDate;
            (*d).tv = f64::NAN;
            (*d)._pad = 0;
            (*d).prototype = prototype;
        }
        ptr
    }

    pub unsafe fn tv(ptr: *mut u8) -> f64 {
        unsafe { (*(ptr as *mut RuneDate)).tv }
    }

    pub unsafe fn set_tv(ptr: *mut u8, tv: f64) {
        unsafe {
            (*(ptr as *mut RuneDate)).tv = tv;
        }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneDate)).prototype }
    }

    pub unsafe fn set_prototype(ptr: *mut u8, proto: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneDate)).prototype = proto;
        }
    }
}

// ---------------------------------------------------------------------------
// §21.4.1 Abstract date-time operations. This implementation uses the
// spec-conformant UTC-only time zone (SystemTimeZoneIdentifier returns "UTC",
// GetNamedTimeZoneOffsetNanoseconds returns 0), so LocalTime(tv) = tv and
// UTC(t) = t.
// ---------------------------------------------------------------------------

pub const MS_PER_DAY: f64 = 86_400_000.0;
const MS_PER_HOUR: f64 = 3_600_000.0;
const MS_PER_MINUTE: f64 = 60_000.0;
const MS_PER_SECOND: f64 = 1000.0;
const MAX_TIME_VALUE: f64 = 8_640_000_000_000_000.0;

/// §21.4.1.3 Day ( tv )
pub fn day(tv: f64) -> i64 {
    (tv / MS_PER_DAY).floor() as i64
}

/// §21.4.1.4 TimeWithinDay ( tv )
pub fn time_within_day(tv: f64) -> f64 {
    tv.rem_euclid(MS_PER_DAY)
}

/// §21.4.1.5 DayFromYear ( y )
pub fn day_from_year(y: i64) -> i64 {
    let ny1 = y - 1970;
    let ny4 = (y - 1969).div_euclid(4);
    let ny100 = (y - 1901).div_euclid(100);
    let ny400 = (y - 1601).div_euclid(400);
    365 * ny1 + ny4 - ny100 + ny400
}

/// §21.4.1.6 TimeFromYear ( y )
pub fn time_from_year(y: i64) -> f64 {
    MS_PER_DAY * day_from_year(y) as f64
}

/// §21.4.1.7 YearFromTime ( tv )
pub fn year_from_time(tv: f64) -> i64 {
    // Binary search: the largest y such that TimeFromYear(y) <= tv.
    let mut lo = -400_000_i64;
    let mut hi = 400_000_i64;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if time_from_year(mid) <= tv {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// §21.4.1.8 DayWithinYear ( tv )
pub fn day_within_year(tv: f64) -> i64 {
    day(tv) - day_from_year(year_from_time(tv))
}

/// §21.4.1.9 InLeapYear ( tv )
pub fn in_leap_year(tv: f64) -> i64 {
    let y = year_from_time(tv);
    if y.rem_euclid(400) == 0 {
        1
    } else if y.rem_euclid(100) == 0 {
        0
    } else if y.rem_euclid(4) == 0 {
        1
    } else {
        0
    }
}

/// §21.4.1.10 MonthFromTime ( tv )
pub fn month_from_time(tv: f64) -> i64 {
    let leap = in_leap_year(tv);
    let dwy = day_within_year(tv);
    if dwy < 31 {
        0
    } else if dwy < 59 + leap {
        1
    } else if dwy < 90 + leap {
        2
    } else if dwy < 120 + leap {
        3
    } else if dwy < 151 + leap {
        4
    } else if dwy < 181 + leap {
        5
    } else if dwy < 212 + leap {
        6
    } else if dwy < 243 + leap {
        7
    } else if dwy < 273 + leap {
        8
    } else if dwy < 304 + leap {
        9
    } else if dwy < 334 + leap {
        10
    } else {
        11
    }
}

/// §21.4.1.11 DateFromTime ( tv )
pub fn date_from_time(tv: f64) -> i64 {
    let leap = in_leap_year(tv);
    let dwy = day_within_year(tv);
    match month_from_time(tv) {
        0 => dwy + 1,
        1 => dwy - 30,
        2 => dwy - 58 - leap,
        3 => dwy - 89 - leap,
        4 => dwy - 119 - leap,
        5 => dwy - 150 - leap,
        6 => dwy - 180 - leap,
        7 => dwy - 211 - leap,
        8 => dwy - 242 - leap,
        9 => dwy - 272 - leap,
        10 => dwy - 303 - leap,
        _ => dwy - 333 - leap,
    }
}

/// §21.4.1.12 WeekDay ( tv )
pub fn week_day(tv: f64) -> i64 {
    (day(tv) + 4).rem_euclid(7)
}

/// §21.4.1.13 HourFromTime ( tv )
pub fn hour_from_time(tv: f64) -> i64 {
    ((tv / MS_PER_HOUR).floor() as i64).rem_euclid(24)
}

/// §21.4.1.14 MinFromTime ( tv )
pub fn min_from_time(tv: f64) -> i64 {
    ((tv / MS_PER_MINUTE).floor() as i64).rem_euclid(60)
}

/// §21.4.1.15 SecFromTime ( tv )
pub fn sec_from_time(tv: f64) -> i64 {
    ((tv / MS_PER_SECOND).floor() as i64).rem_euclid(60)
}

/// §21.4.1.16 MillisecFromTime ( tv )
pub fn millisec_from_time(tv: f64) -> i64 {
    (tv as i64).rem_euclid(1000)
}

/// §21.4.1.26 MakeTime ( hour, min, sec, ms )
pub fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if !hour.is_finite() || !min.is_finite() || !sec.is_finite() || !ms.is_finite() {
        return f64::NAN;
    }
    let h = hour.trunc();
    let m = min.trunc();
    let s = sec.trunc();
    let milli = ms.trunc();
    ((h * MS_PER_HOUR + m * MS_PER_MINUTE) + s * MS_PER_SECOND) + milli
}

/// §21.4.1.27 MakeDay ( year, month, date )
pub fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !year.is_finite() || !month.is_finite() || !date.is_finite() {
        return f64::NAN;
    }
    let y = year.trunc();
    let m = month.trunc();
    let dt = date.trunc();
    let ym = y + (m / 12.0).floor();
    if !ym.is_finite() {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0) as i64;
    // Days before the start of month mn in year y_int (non-leap baseline).
    let y_int = ym as i64;
    let month_starts: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_in_year = month_starts[mn as usize];
    let leap = if y_int.rem_euclid(400) == 0 {
        1
    } else if y_int.rem_euclid(100) == 0 {
        0
    } else if y_int.rem_euclid(4) == 0 {
        1
    } else {
        0
    };
    if mn >= 2 {
        day_in_year += leap;
    }
    let tv_first = time_from_year(y_int) + (day_in_year as f64) * MS_PER_DAY;
    (tv_first / MS_PER_DAY).floor() + dt - 1.0
}

/// §21.4.1.28 MakeDate ( day, time )
pub fn make_date(day: f64, time: f64) -> f64 {
    if !day.is_finite() || !time.is_finite() {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if !tv.is_finite() {
        return f64::NAN;
    }
    tv
}

/// §21.4.1.29 MakeFullYear ( year )
pub fn make_full_year(year: f64) -> f64 {
    if !year.is_finite() {
        return f64::NAN;
    }
    let truncated = year.trunc();
    if (0.0..=99.0).contains(&truncated) {
        return 1900.0 + truncated;
    }
    truncated
}

/// §21.4.1.30 TimeClip ( time )
pub fn time_clip(time: f64) -> f64 {
    if !time.is_finite() {
        return f64::NAN;
    }
    if time.abs() > MAX_TIME_VALUE {
        return f64::NAN;
    }
    time.trunc()
}

/// ToIntegerOrInfinity (§7.1.5) applied by MakeTime/MakeDay.
/// MakeTime/MakeDay use `trunc()` directly (equivalent for finite inputs).
///
/// Current time as a UTC time value (milliseconds since the epoch).
///
/// Uses the system clock; the result is a real time value (not a test mock).
pub fn now_ms() -> f64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    }
}

// ---------------------------------------------------------------------------
// §21.4.4.41 String formatting helpers.
// ---------------------------------------------------------------------------

pub fn zero_padded(v: i64, width: usize) -> String {
    let s = v.abs().to_string();
    if s.len() >= width {
        s
    } else {
        format!("{}{}", "0".repeat(width - s.len()), s)
    }
}

const WEEKDAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Table 61: name of the weekday at index 0..6.
pub fn weekday_name(idx: i64) -> &'static str {
    WEEKDAY_NAMES[idx.rem_euclid(7) as usize]
}

/// Table 62: name of the month at index 0..11.
pub fn month_name(idx: i64) -> &'static str {
    MONTH_NAMES[idx.rem_euclid(12) as usize]
}

/// §21.4.4.41.1 TimeString ( tv ) — "HH:mm:ss GMT"
pub fn time_string(tv: f64) -> String {
    format!(
        "{}:{}:{} GMT",
        zero_padded(hour_from_time(tv), 2),
        zero_padded(min_from_time(tv), 2),
        zero_padded(sec_from_time(tv), 2)
    )
}

/// §21.4.4.41.2 DateString ( tv ) — "Wed Aug 19 2026"
pub fn date_string(tv: f64) -> String {
    let yv = year_from_time(tv);
    let year_sign = if yv >= 0 { "" } else { "-" };
    format!(
        "{} {} {} {}{}",
        WEEKDAY_NAMES[week_day(tv) as usize],
        MONTH_NAMES[month_from_time(tv) as usize],
        zero_padded(date_from_time(tv), 2),
        year_sign,
        zero_padded(yv, 4)
    )
}

/// §21.4.4.41.3 TimeZoneString ( tv ) — "+0000" (UTC-only implementation).
pub fn time_zone_string(_tv: f64) -> String {
    "+0000".to_string()
}

/// §21.4.4.41.4 ToDateString ( tv ) — full `toString` result.
pub fn to_date_string(tv: f64) -> String {
    if tv.is_nan() {
        return "Invalid Date".to_string();
    }
    let ts = format!("{}{}", time_string(tv), time_zone_string(tv));
    format!("{} {}", date_string(tv), ts)
}

/// §21.4.4.36 toISOString — "YYYY-MM-DDTHH:mm:ss.sssZ" on the UTC scale.
/// Returns None if tv is NaN or the year cannot be represented in the format.
pub fn to_iso_string(tv: f64) -> Option<String> {
    if tv.is_nan() {
        return None;
    }
    let y = year_from_time(tv);
    let (year_str, negative) = if (0..=9999).contains(&y) {
        (zero_padded(y, 4), false)
    } else {
        // Expanded year: sign + 6 digits (year 0 is +000000).
        (zero_padded(y, 6), y < 0)
    };
    let sign = if negative {
        "-"
    } else if !(0..=9999).contains(&y) {
        "+"
    } else {
        ""
    };
    Some(format!(
        "{}{}-{}-{}T{}:{}:{}.{}Z",
        sign,
        year_str,
        zero_padded(month_from_time(tv) + 1, 2),
        zero_padded(date_from_time(tv), 2),
        zero_padded(hour_from_time(tv), 2),
        zero_padded(min_from_time(tv), 2),
        zero_padded(sec_from_time(tv), 2),
        zero_padded(millisec_from_time(tv), 3)
    ))
}

// ---------------------------------------------------------------------------
// §21.4.1.31 Date Time String Format parsing (Date.parse).
// ---------------------------------------------------------------------------

/// Parse a string in the Date Time String Format (§21.4.1.31).
/// Returns the UTC time value, or NaN if the string does not conform.
/// Date-only forms are interpreted as UTC; date-time forms without an offset
/// are interpreted as local time (equal to UTC in this implementation).
pub fn parse_date_time_string(s: &str) -> f64 {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;

    // Year: 4 digits (0000-9999) or sign + 6 digits (expanded).
    let (mut year, neg) = match bytes.first() {
        Some(b'+') | Some(b'-') => {
            let neg = bytes[0] == b'-';
            i = 1;
            let (y, ok) = parse_digits(bytes, &mut i, 6);
            if !ok {
                return f64::NAN;
            }
            (y, neg)
        }
        _ => {
            let (y, ok) = parse_digits(bytes, &mut i, 4);
            if !ok {
                return f64::NAN;
            }
            (y, false)
        }
    };
    if neg {
        if year == 0 {
            return f64::NAN; // "-000000" is invalid
        }
        year = -year;
    }

    let mut month = 1i64;
    let mut day = 1i64;
    if i < n && bytes[i] == b'-' {
        i += 1;
        let (m, ok) = parse_digits(bytes, &mut i, 2);
        if !ok || !(1..=12).contains(&m) {
            return f64::NAN;
        }
        month = m;
        if i < n && bytes[i] == b'-' {
            i += 1;
            let (d, ok) = parse_digits(bytes, &mut i, 2);
            if !ok || !(1..=31).contains(&d) {
                return f64::NAN;
            }
            day = d;
        }
    }
    // Date-time forms: the date-only form immediately followed by "T" and a
    // time form. "YYYY" alone is valid; "YYYY-MM" alone is valid.
    if i < n && bytes[i] == b'T' {
        i += 1;
    } else {
        if i != n {
            return f64::NAN;
        }
        return date_to_time_value(year, month, day, 0, 0, 0, 0.0);
    }
    let (hour, ok) = parse_digits(bytes, &mut i, 2);
    if !ok || hour > 24 {
        return f64::NAN;
    }
    let mut minute = 0i64;
    let mut second = 0i64;
    let mut ms = 0.0;
    if i < n && bytes[i] == b':' {
        i += 1;
        let (min, ok) = parse_digits(bytes, &mut i, 2);
        if !ok || min > 59 {
            return f64::NAN;
        }
        minute = min;
        if i < n && bytes[i] == b':' {
            i += 1;
            let (sec, ok) = parse_digits(bytes, &mut i, 2);
            if !ok || sec > 59 {
                return f64::NAN;
            }
            second = sec;
            if i < n && bytes[i] == b'.' {
                i += 1;
                let start = i;
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start || i - start > 9 {
                    return f64::NAN;
                }
                let frac = &s[start..i];
                let mut millis = String::from(frac);
                millis.push_str(&"0".repeat(9 - millis.len()));
                ms = millis.parse::<f64>().unwrap_or(0.0) / 1_000_000.0;
            }
        }
    }
    // Hour 24 is only valid as 24:00:00 (the end of the day).
    if hour == 24 && (minute != 0 || second != 0 || ms != 0.0) {
        return f64::NAN;
    }

    // Optional UTC offset.
    let mut offset_ms = 0f64;
    if i < n {
        let sign = match bytes[i] {
            b'Z' => {
                i += 1;
                0i64
            }
            b'+' | b'-' => {
                let neg = bytes[i] == b'-';
                i += 1;
                let (oh, ok) = parse_digits(bytes, &mut i, 2);
                if !ok || oh > 23 {
                    return f64::NAN;
                }
                if i < n && bytes[i] == b':' {
                    i += 1;
                }
                let (om, ok) = parse_digits(bytes, &mut i, 2);
                if !ok || om > 59 {
                    return f64::NAN;
                }
                if neg {
                    -(oh * 60 + om) * 60_000
                } else {
                    (oh * 60 + om) * 60_000
                }
            }
            _ => return f64::NAN,
        };
        if sign != 0 {
            offset_ms = sign as f64;
        }
    }
    if i != n {
        return f64::NAN;
    }

    let utc = date_to_time_value(year, month, day, hour, minute, second, ms) - offset_ms;
    time_clip(utc)
}

/// Parse a date string: the Date Time String Format first, then the legacy
/// `toString` format as an implementation-specific heuristic.
pub fn parse_date_string(s: &str) -> f64 {
    let tv = parse_date_time_string(s);
    if !tv.is_nan() {
        return tv;
    }
    parse_legacy_to_string_format(s)
}

/// Legacy fallback for the `toString` output format
/// ("Wed Aug 19 2026 12:34:56 GMT+0000" — §21.4.3.2 allows implementation
/// heuristics for strings not in the Date Time String Format). Returns NaN
/// unless the string matches exactly.
fn parse_legacy_to_string_format(s: &str) -> f64 {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() != 6 {
        return f64::NAN;
    }
    if !WEEKDAY_NAMES.contains(&tokens[0]) {
        return f64::NAN;
    }
    let Some(month_idx) = MONTH_NAMES.iter().position(|m| *m == tokens[1]) else {
        return f64::NAN;
    };
    let month = (month_idx + 1) as i64;
    let Ok(day) = tokens[2].parse::<i64>() else {
        return f64::NAN;
    };
    let Ok(year) = tokens[3].parse::<i64>() else {
        return f64::NAN;
    };
    let parts: Vec<&str> = tokens[4].split(':').collect();
    if parts.len() != 3 {
        return f64::NAN;
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (
        parts[0].parse::<i64>(),
        parts[1].parse::<i64>(),
        parts[2].parse::<i64>(),
    ) else {
        return f64::NAN;
    };
    let offset_tok = match tokens[5].strip_prefix("GMT") {
        Some("") => "+0000",
        Some(rest) => rest,
        None => return f64::NAN,
    };
    if offset_tok.len() != 5 || !offset_tok.starts_with('+') && !offset_tok.starts_with('-') {
        return f64::NAN;
    }
    let (Ok(oh), Ok(om)) = (
        offset_tok[1..3].parse::<i64>(),
        offset_tok[3..5].parse::<i64>(),
    ) else {
        return f64::NAN;
    };
    let offset_ms = if offset_tok.starts_with('-') {
        -(oh * 60 + om) * 60_000
    } else {
        (oh * 60 + om) * 60_000
    };
    // The displayed local time is tv + offset; recover tv by subtracting.
    let local = date_to_time_value(year, month, day, hour, minute, second, 0.0);
    time_clip(local - offset_ms as f64)
}

fn parse_digits(bytes: &[u8], i: &mut usize, count: usize) -> (i64, bool) {
    if *i + count > bytes.len() {
        return (0, false);
    }
    let mut v: i64 = 0;
    for _ in 0..count {
        let b = bytes[*i];
        if !b.is_ascii_digit() {
            return (0, false);
        }
        v = v * 10 + (b - b'0') as i64;
        *i += 1;
    }
    (v, true)
}

/// Assemble a UTC time value from calendar components (§21.4.1.17 / MakeDay+MakeTime).
/// Values are pre-validated; the calendar math folds months/days/hours outside
/// their ranges the same way MakeDay/MakeTime do (e.g. month 13 → next year).
fn date_to_time_value(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    ms: f64,
) -> f64 {
    let d = make_day(year as f64, (month - 1) as f64, day as f64);
    let t = make_time(hour as f64, minute as f64, second as f64, ms);
    time_clip(make_date(d, t))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epoch() {
        assert_eq!(day(0.0), 0);
        assert_eq!(time_within_day(0.0), 0.0);
        assert_eq!(year_from_time(0.0), 1970);
        assert_eq!(month_from_time(0.0), 0);
        assert_eq!(date_from_time(0.0), 1);
        assert_eq!(week_day(0.0), 4); // Thursday
        assert_eq!(hour_from_time(0.0), 0);
    }

    #[test]
    fn test_known_dates() {
        // 2026-08-19 12:34:56 UTC = ?
        let tv = make_date(
            make_day(2026.0, 7.0, 19.0),
            make_time(12.0, 34.0, 56.0, 0.0),
        );
        assert_eq!(year_from_time(tv), 2026);
        assert_eq!(month_from_time(tv), 7);
        assert_eq!(date_from_time(tv), 19);
        assert_eq!(hour_from_time(tv), 12);
        assert_eq!(min_from_time(tv), 34);
        assert_eq!(sec_from_time(tv), 56);
        // Leap year: 2024-02-29 exists.
        let leap = make_date(make_day(2024.0, 1.0, 29.0), 0.0);
        assert_eq!(month_from_time(leap), 1);
        assert_eq!(date_from_time(leap), 29);
        // 2023-02-29 rolls over to March 1 (MakeDay folding).
        let roll = make_date(make_day(2023.0, 1.0, 29.0), 0.0);
        assert_eq!(month_from_time(roll), 2);
        assert_eq!(date_from_time(roll), 1);
    }

    #[test]
    fn test_make_day_folding() {
        // month 12 (Jan of next year), month -1 (Dec of previous year).
        let tv = make_date(make_day(2026.0, 12.0, 1.0), 0.0);
        assert_eq!(year_from_time(tv), 2027);
        assert_eq!(month_from_time(tv), 0);
        let tv2 = make_date(make_day(2026.0, -1.0, 1.0), 0.0);
        assert_eq!(year_from_time(tv2), 2025);
        assert_eq!(month_from_time(tv2), 11);
    }

    #[test]
    fn test_time_clip() {
        assert!(time_clip(f64::NAN).is_nan());
        assert!(time_clip(f64::INFINITY).is_nan());
        assert!(time_clip(8.64e15 + 1.0).is_nan());
        assert!(time_clip(-8.64e15 - 1.0).is_nan());
        assert_eq!(time_clip(123.9), 123.0);
    }

    #[test]
    fn test_to_date_string() {
        let tv = make_date(
            make_day(2026.0, 7.0, 19.0),
            make_time(12.0, 34.0, 56.0, 789.0),
        );
        assert_eq!(to_date_string(tv), "Wed Aug 19 2026 12:34:56 GMT+0000");
        assert_eq!(to_date_string(f64::NAN), "Invalid Date");
    }

    #[test]
    fn test_to_iso_string() {
        let tv = make_date(
            make_day(2026.0, 7.0, 19.0),
            make_time(12.0, 34.0, 56.0, 789.0),
        );
        assert_eq!(to_iso_string(tv).unwrap(), "2026-08-19T12:34:56.789Z");
        assert!(to_iso_string(f64::NAN).is_none());
        // Expanded year.
        let tv2 = make_date(make_day(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(to_iso_string(tv2).unwrap(), "-000001-01-01T00:00:00.000Z");
        let tv3 = make_date(make_day(10000.0, 0.0, 1.0), 0.0);
        assert_eq!(to_iso_string(tv3).unwrap(), "+010000-01-01T00:00:00.000Z");
        // 1970-01-01T00:00:00Z
        assert_eq!(to_iso_string(0.0).unwrap(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn test_parse_iso() {
        let tv = parse_date_time_string("2026-08-19T12:34:56.789Z");
        assert_eq!(to_iso_string(tv).unwrap(), "2026-08-19T12:34:56.789Z");
        assert_eq!(parse_date_time_string("1970-01-01T00:00:00Z"), 0.0);
        // Date-only forms (UTC).
        assert_eq!(
            parse_date_time_string("2026"),
            parse_date_time_string("2026-01-01")
        );
        assert_eq!(
            parse_date_time_string("2026-08"),
            parse_date_time_string("2026-08-01")
        );
        // No offset → local (UTC here).
        assert_eq!(
            parse_date_time_string("2026-08-19T12:34:56"),
            parse_date_time_string("2026-08-19T12:34:56Z")
        );
        // Offset shifting: local 10:00 with +02:30 offset is 07:30Z.
        assert_eq!(
            parse_date_time_string("2026-08-19T10:00:00+02:30"),
            parse_date_time_string("2026-08-19T07:30:00Z")
        );
        // 24:00 == next day.
        assert_eq!(
            parse_date_time_string("2026-08-19T24:00"),
            parse_date_time_string("2026-08-20T00:00Z")
        );
        // Invalid forms → NaN.
        assert!(parse_date_time_string("garbage").is_nan());
        assert!(parse_date_time_string("2026-13-01").is_nan());
        assert!(parse_date_time_string("2026-08-32").is_nan());
        assert!(parse_date_time_string("2026-08-19T25:00").is_nan());
        assert!(parse_date_time_string("2026-08-19T24:30").is_nan());
        assert!(parse_date_time_string("2026-08-19T12:60").is_nan());
        assert!(parse_date_time_string("-000000-01-01").is_nan());
        assert!(parse_date_time_string("2026-08-19T12:34:56.1234567890").is_nan());
        // Expanded years.
        assert_eq!(
            parse_date_time_string("-000001-01-01T00:00:00Z"),
            make_date(make_day(-1.0, 0.0, 1.0), 0.0)
        );
        // -271821-04-20T00:00:00Z is exactly the earliest time value (valid);
        // years beyond the range are out of bounds (NaN).
        assert_eq!(parse_date_time_string("-271821-04-20T00:00:00Z"), -8.64e15);
        assert!(parse_date_time_string("-271822-01-01T00:00:00Z").is_nan());
        assert!(parse_date_time_string("+275761-01-01T00:00:00Z").is_nan());
    }

    #[test]
    fn test_parse_roundtrip() {
        let tv = make_date(
            make_day(1995.0, 1.0, 4.0),
            make_time(23.0, 59.0, 59.0, 999.0),
        );
        let iso = to_iso_string(tv).unwrap();
        assert_eq!(parse_date_time_string(&iso), tv);
        let s = to_date_string(tv);
        // toString has no millis; parse back loses .999ms.
        let parsed = parse_date_string(&s);
        let expected = make_date(make_day(1995.0, 1.0, 4.0), make_time(23.0, 59.0, 59.0, 0.0));
        assert_eq!(parsed, expected);
        // Round-trip through ISO preserves the millis.
        let iso = to_iso_string(tv).unwrap();
        assert_eq!(parse_date_string(&iso), tv);
    }
}
