//! The value words of the filter text form, parsed strictly: a number is `[+-]digits[.digits]`
//! (no exponent, no `inf`), and every unit is written in the one spelling given here.

use super::{Channels, FilterError};
use crossfade_core::ChannelLayout;

/// Longest time a filter takes, in seconds; keeps every frame position far inside `u64`.
pub const MAX_SECONDS: f64 = 1e9;
pub const MAX_RATE: u32 = 1_048_575;
pub const MAX_CHANNELS: u16 = 64;

pub(super) const TIME: &str = "expected a time such as 10s, 500ms or 1:30";
pub(super) const DB: &str = "expected a level in dB, such as -3dB";
pub(super) const LUFS: &str = "expected a loudness in LUFS, such as -16LUFS";
pub(super) const DBTP: &str = "expected a true peak in dBTP, such as -1dBTP";
pub(super) const RATE: &str = "expected a sample rate such as 48000, 48000Hz or 44.1kHz";

pub(super) fn channels_expected() -> String {
    format!(
        "expected a layout ({}) or a channel count from 1 to {MAX_CHANNELS}",
        ChannelLayout::NAMES.join(", ")
    )
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `[+-]digits[.digits]`, nothing else.
pub(super) fn number(s: &str) -> Option<f64> {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    if !is_digits(int) || frac.is_some_and(|f| !is_digits(f)) {
        return None;
    }
    s.parse().ok()
}

fn with_unit(s: &str, unit: &str) -> Option<f64> {
    number(s.strip_suffix(unit)?)
}

/// Seconds, unchecked for range (see [`check_seconds`]).
pub(super) fn time(s: &str) -> Option<f64> {
    if s.contains(':') {
        return clock(s);
    }
    if let Some(ms) = s.strip_suffix("ms") {
        return number(ms).map(|v| v / 1000.0);
    }
    with_unit(s, "s")
}

/// `m:ss[.f]` or `h:mm:ss[.f]`; minutes and seconds below 60 and two digits after a colon.
fn clock(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [m, sec] => ("0", *m, *sec),
        [h, m, sec] if m.len() == 2 => (*h, *m, *sec),
        _ => return None,
    };
    let (whole, frac) = seconds.split_once('.').unwrap_or((seconds, "0"));
    if !(is_digits(hours) && is_digits(minutes) && whole.len() == 2 && is_digits(whole)) {
        return None;
    }
    if !is_digits(frac) {
        return None;
    }
    let (h, m): (f64, f64) = (hours.parse().ok()?, minutes.parse().ok()?);
    let sec: f64 = seconds.parse().ok()?;
    if (parts.len() == 3 && m >= 60.0) || sec >= 60.0 {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// `48000`, `48000Hz`, `48kHz` or `44.1kHz`: a whole number of hertz, exactly.
pub(super) fn rate(s: &str) -> Option<u32> {
    if let Some(k) = s.strip_suffix("kHz") {
        let (int, frac) = k.split_once('.').unwrap_or((k, ""));
        if !is_digits(int) || frac.len() > 3 || (!frac.is_empty() && !is_digits(frac)) {
            return None;
        }
        let frac: u64 = format!("{frac:0<3}").parse().ok()?;
        let hz = int
            .parse::<u64>()
            .ok()?
            .checked_mul(1000)?
            .checked_add(frac)?;
        return u32::try_from(hz).ok();
    }
    let digits = s.strip_suffix("Hz").unwrap_or(s);
    if !is_digits(digits) {
        return None;
    }
    digits.parse().ok()
}

pub(super) fn channels(s: &str) -> Option<Channels> {
    if let Some(layout) = ChannelLayout::from_name(s) {
        return Some(Channels::Layout(layout));
    }
    if !is_digits(s) {
        return None;
    }
    s.parse().ok().map(Channels::Count)
}

/// Rows of `,`-separated numbers, `/` between rows.
pub(super) fn matrix(s: &str) -> Option<Vec<Vec<f64>>> {
    s.split('/')
        .map(|row| row.split(',').map(number).collect())
        .collect()
}

pub(super) fn check_seconds(x: f64) -> Result<(), String> {
    if x < 0.0 {
        Err("a time cannot be negative".into())
    } else if x <= MAX_SECONDS {
        Ok(())
    } else {
        Err(format!("a time must be at most {MAX_SECONDS}s"))
    }
}

fn word<T>(s: &str, parsed: Option<T>, expected: &str) -> Result<T, FilterError> {
    parsed.ok_or_else(|| FilterError::at(s, expected))
}

/// A level such as `-6dB` (what `--gain` takes).
pub fn parse_db(s: &str) -> Result<f64, FilterError> {
    word(s, with_unit(s, "dB"), DB)
}

/// A time such as `10s`, `500ms`, `1:30` or `1:02:03.5` (what `--start` and the fades take),
/// in seconds.
pub fn parse_time(s: &str) -> Result<f64, FilterError> {
    let t = word(s, time(s), TIME)?;
    check_seconds(t).map_err(|m| FilterError::at(s, m))?;
    Ok(t)
}

/// A loudness such as `-16LUFS` (what `--normalize` takes).
pub fn parse_lufs(s: &str) -> Result<f64, FilterError> {
    word(s, with_unit(s, "LUFS"), LUFS)
}

/// A true peak such as `-1dBTP` (what `--true-peak` takes).
pub fn parse_dbtp(s: &str) -> Result<f64, FilterError> {
    word(s, with_unit(s, "dBTP"), DBTP)
}

/// A sample rate such as `48000`, `48000Hz` or `44.1kHz` (what `--sample-rate` takes).
pub fn parse_rate(s: &str) -> Result<u32, FilterError> {
    let r = word(s, rate(s), RATE)?;
    super::check_rate(r).map_err(|m| FilterError::at(s, m))?;
    Ok(r)
}

/// A layout name or a channel count (what `--channels` takes).
pub fn parse_channels(s: &str) -> Result<Channels, FilterError> {
    let c = channels(s).ok_or_else(|| FilterError::at(s, channels_expected()))?;
    c.check().map_err(|m| FilterError::at(s, m))?;
    Ok(c)
}
