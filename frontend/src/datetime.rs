//! What the date field holds, and what the API takes.

/// Minutes from UTC, east positive. `Date::getTimezoneOffset` reports the
/// opposite sign — Tokyo answers `-540` — so it is negated here.
pub fn local_offset_minutes() -> i32 {
    -(js_sys::Date::new_0().get_timezone_offset() as i32)
}

/// `2026-08-09T14:30` as `2026-08-09T14:30:00+09:00`, or `None` for anything a
/// `datetime-local` input would not produce, including the empty string.
pub fn to_rfc3339(local: &str, offset_minutes: i32) -> Option<String> {
    let local = local.trim();
    let stamp = match local.len() {
        16 => format!("{local}:00"),
        // Some browsers add seconds.
        19 => local.to_string(),
        _ => return None,
    };
    // Right length is not right shape: `2026-08-09X14:30` is 16 characters.
    let bytes = stamp.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    let punctuation = |i: usize| separators.iter().any(|(at, _)| *at == i);
    if !separators.iter().all(|(at, c)| bytes[*at] == *c)
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| punctuation(i) || b.is_ascii_digit())
    {
        return None;
    }
    let (sign, minutes) = if offset_minutes < 0 {
        ('-', -offset_minutes)
    } else {
        ('+', offset_minutes)
    };
    Some(format!(
        "{stamp}{sign}{:02}:{:02}",
        minutes / 60,
        minutes % 60
    ))
}

/// The server enforces this too; here it only saves a round trip.
pub fn is_future(rfc3339: &str) -> bool {
    let at = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(rfc3339)).get_time();
    !at.is_nan() && at > js_sys::Date::now()
}

/// The reverse of [`to_rfc3339`], in the visitor's zone and cut to the minute.
/// By hand rather than through `Date`, so it can be tested off a browser.
pub fn from_rfc3339(stamp: &str, offset_minutes: i32) -> Option<String> {
    let (date, time) = stamp.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let mut clock = time.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.get(..2)?.parse().ok()?;

    let total =
        days_from_civil(year, month, day) * 1440 + hour * 60 + minute + offset_minutes as i64;
    let (days, minutes) = (total.div_euclid(1440), total.rem_euclid(1440));
    let (y, m, d) = civil_from_days(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}",
        minutes / 60,
        minutes % 60
    ))
}

/// What the picker shows for a canonical `YYYY-MM-DDTHH:MM`: `2026/08/20 01:54`.
pub fn to_display(local: &str) -> Option<String> {
    let (date, time) = local.trim().split_once('T')?;
    let hhmm = time.get(..5)?;
    if date.len() != 10 {
        return None;
    }
    Some(format!(
        "{}/{}/{} {hhmm}",
        &date[..4],
        &date[5..7],
        &date[8..10]
    ))
}

/// The reverse, for a date typed into the field rather than picked. Lenient
/// about width — `2026/8/2 1:4` is unambiguous — strict about the calendar.
pub fn from_display(text: &str) -> Option<String> {
    let (date, time) = text.trim().split_once(' ')?;
    let mut parts = date.split('/');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let mut clock = time.trim().split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    if parts.next().is_some() || clock.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}"
    ))
}

/// What a field may hold once it is finished, given the fields before it.
/// Only the day needs its predecessors, and only because February does.
fn field_bounds(index: usize, done: &[i64]) -> Option<(i64, i64)> {
    match index {
        1 => Some((1, 12)),
        2 => Some((1, days_in_month(done[0], done[1]))),
        3 => Some((0, 23)),
        4 => Some((0, 59)),
        _ => None,
    }
}

/// Punctuates and pads a half-typed date, and pulls a finished field into
/// range — but never a half-typed one, since `2026/1` is on its way to October.
/// Pasted text goes through the same mill, whatever punctuated it.
pub fn mask_display(input: &str) -> String {
    const FIELDS: [(usize, Option<char>); 5] = [
        (4, Some('/')),
        (2, Some('/')),
        (2, Some(' ')),
        (2, Some(':')),
        (2, None),
    ];
    let mut out = String::new();
    let mut digits = String::new();
    let mut done: Vec<i64> = Vec::new();
    let mut fields = FIELDS.iter();
    let Some(mut current) = fields.next() else {
        return out;
    };
    for c in input.chars() {
        let (width, separator) = *current;
        if c.is_ascii_digit() {
            digits.push(c);
            if digits.len() < width {
                continue;
            }
        } else if digits.is_empty() {
            // A separator with nothing before it closes nothing.
            continue;
        }
        let mut value: i64 = digits.parse().unwrap_or(0);
        if let Some((low, high)) = field_bounds(done.len(), &done) {
            value = value.clamp(low, high);
        }
        out.push_str(&format!("{value:0width$}"));
        done.push(value);
        digits.clear();
        let Some(next) = fields.next() else { break };
        if let Some(separator) = separator {
            out.push(separator);
        }
        current = next;
    }
    out.push_str(&digits);
    out
}

pub fn days_in_month(year: i64, month: i64) -> i64 {
    let (next_year, next_month) = add_months(year, month, 1);
    days_from_civil(next_year, next_month, 1) - days_from_civil(year, month, 1)
}

/// Sunday is 0, matching the order the grid's header is written in.
pub fn weekday(year: i64, month: i64, day: i64) -> i64 {
    (days_from_civil(year, month, day) + 4).rem_euclid(7)
}

pub fn add_months(year: i64, month: i64, delta: i64) -> (i64, i64) {
    let total = year * 12 + (month - 1) + delta;
    (total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// The six weeks covering `month`, padded with the months either side so the
/// popover is always the same height.
pub fn month_grid(year: i64, month: i64) -> Vec<(i64, i64, i64)> {
    let start = days_from_civil(year, month, 1) - weekday(year, month, 1);
    (0..42).map(|i| civil_from_days(start + i)).collect()
}

/// Today in the browser's own zone.
pub fn today_local() -> (i64, i64, i64) {
    let now = js_sys::Date::now() as i64;
    let minutes = (now / 60_000) + local_offset_minutes() as i64;
    civil_from_days(minutes.div_euclid(1440))
}

/// Days since 1970-01-01, by Howard Hinnant's civil-calendar algorithm — no
/// leap-year special case to forget.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The separators arrive on their own as the field fills up.
    #[test]
    fn typing_is_punctuated_as_it_goes() {
        assert_eq!(mask_display("2026"), "2026/");
        assert_eq!(mask_display("2026/"), "2026/");
        assert_eq!(mask_display("2026/0"), "2026/0");
        assert_eq!(mask_display("2026/04"), "2026/04/");
        assert_eq!(mask_display("2026/04/12"), "2026/04/12 ");
        assert_eq!(mask_display("2026/04/12 18"), "2026/04/12 18:");
        assert_eq!(mask_display("2026/04/12 18:30"), "2026/04/12 18:30");
        assert_eq!(mask_display(""), "");
    }

    /// A separator typed early says the field is finished: `4` means April.
    #[test]
    fn a_short_field_is_padded_when_it_is_cut_short() {
        assert_eq!(mask_display("2026/4/"), "2026/04/");
        assert_eq!(mask_display("2026/4/2 "), "2026/04/02 ");
        assert_eq!(mask_display("2026/4/2 8:"), "2026/04/02 08:");
        assert_eq!(mask_display("26/"), "0026/");
    }

    /// Pasted text is punctuated however its source felt like it.
    #[test]
    fn a_pasted_stamp_is_rewritten_into_this_fields_shape() {
        assert_eq!(mask_display("2026-04-12 18:30"), "2026/04/12 18:30");
        assert_eq!(mask_display("2026-04-12T18:30"), "2026/04/12 18:30");
        assert_eq!(mask_display("202604121830"), "2026/04/12 18:30");
        assert_eq!(mask_display("2026-04-12T18:30:45.000Z"), "2026/04/12 18:30");
    }

    /// A finished field cannot say something the calendar or the clock does
    /// not have, so it is pulled to the nearest thing they do.
    #[test]
    fn an_out_of_range_field_is_pulled_into_range() {
        assert_eq!(mask_display("2028/18/24 88:88"), "2028/12/24 23:59");
        assert_eq!(mask_display("2026/00/00 "), "2026/01/01 ");
        assert_eq!(mask_display("2026/02/31 "), "2026/02/28 ");
        assert_eq!(mask_display("2028/02/31 "), "2028/02/29 ");
        assert_eq!(mask_display("2026/04/31 "), "2026/04/30 ");
    }

    /// Only once the field is finished: a lone `1` is still on its way to
    /// October, and clamping it to January would make that untypable.
    #[test]
    fn a_half_typed_field_is_left_where_it_is() {
        assert_eq!(mask_display("2026/1"), "2026/1");
        assert_eq!(mask_display("2026/04/3"), "2026/04/3");
        assert_eq!(mask_display("2026/04/12 8"), "2026/04/12 8");
    }

    /// Anything past the minutes is not part of a date this field holds.
    #[test]
    fn trailing_rubbish_is_dropped() {
        assert_eq!(mask_display("2026/04/12 18:3099"), "2026/04/12 18:30");
        assert_eq!(mask_display("nonsense"), "");
    }

    #[test]
    fn the_input_shows_a_date_the_way_this_app_writes_them() {
        assert_eq!(
            to_display("2026-08-20T01:54").as_deref(),
            Some("2026/08/20 01:54")
        );
        assert_eq!(to_display("").as_deref(), None);
    }

    /// Typing is allowed, so the display format has to parse back.
    #[test]
    fn a_typed_date_comes_back_as_a_canonical_one() {
        assert_eq!(
            from_display("2026/08/20 01:54").as_deref(),
            Some("2026-08-20T01:54")
        );
        assert_eq!(
            from_display("  2026/08/20 01:54 ").as_deref(),
            Some("2026-08-20T01:54")
        );
        assert_eq!(
            from_display("2026/8/2 1:4").as_deref(),
            Some("2026-08-02T01:04")
        );
        assert_eq!(from_display("2026/08/20"), None);
        assert_eq!(from_display("2026/13/20 01:54"), None);
        assert_eq!(from_display("2026/02/30 01:54"), None);
        assert_eq!(from_display("2026/08/20 24:00"), None);
        assert_eq!(from_display("2026/08/20 01:60"), None);
        assert_eq!(from_display("nonsense"), None);
    }

    #[test]
    fn a_month_knows_its_own_length() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2026, 8), 31);
        assert_eq!(days_in_month(2026, 4), 30);
    }

    #[test]
    fn weekdays_are_sunday_first() {
        assert_eq!(weekday(1970, 1, 1), 4);
        assert_eq!(weekday(2026, 8, 20), 4);
        assert_eq!(weekday(2026, 8, 23), 0);
    }

    /// Stepping past December has to carry the year, in both directions.
    #[test]
    fn stepping_a_month_carries_the_year() {
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 8, 0), (2026, 8));
    }

    /// Six full weeks always, so the popover never changes height.
    #[test]
    fn the_grid_is_six_weeks_padded_from_the_months_either_side() {
        let grid = month_grid(2026, 8);
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0], (2026, 7, 26));
        assert_eq!(grid[6], (2026, 8, 1));
        assert_eq!(grid[36], (2026, 8, 31));
        assert_eq!(grid[37], (2026, 9, 1));
        assert_eq!(grid[41], (2026, 9, 5));
        // February 2026 starts on a Sunday: no padding at the front.
        assert_eq!(month_grid(2026, 2)[0], (2026, 2, 1));
    }

    /// A half-hour zone is where a naive "whole hours" conversion falls over.
    #[test]
    fn a_local_stamp_carries_its_own_offset() {
        assert_eq!(
            to_rfc3339("2026-08-09T14:30", 540).as_deref(),
            Some("2026-08-09T14:30:00+09:00")
        );
        assert_eq!(
            to_rfc3339("2026-08-09T14:30", -300).as_deref(),
            Some("2026-08-09T14:30:00-05:00")
        );
        assert_eq!(
            to_rfc3339("2026-08-09T14:30", 330).as_deref(),
            Some("2026-08-09T14:30:00+05:30")
        );
        assert_eq!(
            to_rfc3339("2026-08-09T14:30", 0).as_deref(),
            Some("2026-08-09T14:30:00+00:00")
        );
    }

    /// Some browsers hand back seconds; a cleared picker hands back nothing.
    #[test]
    fn seconds_are_accepted_and_nonsense_is_refused() {
        assert_eq!(
            to_rfc3339("2026-08-09T14:30:45", 540).as_deref(),
            Some("2026-08-09T14:30:45+09:00")
        );
        assert_eq!(
            to_rfc3339("  2026-08-09T14:30  ", 540).as_deref(),
            Some("2026-08-09T14:30:00+09:00")
        );
        assert_eq!(to_rfc3339("", 540), None);
        assert_eq!(to_rfc3339("2026-08-09", 540), None);
        assert_eq!(to_rfc3339("whenever at all", 540), None);
        // Right length, wrong shape.
        assert_eq!(to_rfc3339("2026-08-09X14:30", 540), None);
    }

    #[test]
    fn a_stored_stamp_comes_back_as_a_local_one() {
        assert_eq!(
            from_rfc3339("2026-08-09T05:30:00.000Z", 540).as_deref(),
            Some("2026-08-09T14:30")
        );
        assert_eq!(
            from_rfc3339("2026-08-09T20:00:00.000Z", 540).as_deref(),
            Some("2026-08-10T05:00")
        );
        assert_eq!(
            from_rfc3339("2026-08-31T20:00:00.000Z", 540).as_deref(),
            Some("2026-09-01T05:00")
        );
        assert_eq!(
            from_rfc3339("2026-08-09T02:00:00.000Z", -300).as_deref(),
            Some("2026-08-08T21:00")
        );
        // A year boundary and a leap day: where a hand-rolled calendar breaks.
        assert_eq!(
            from_rfc3339("2026-12-31T20:00:00.000Z", 540).as_deref(),
            Some("2027-01-01T05:00")
        );
        assert_eq!(
            from_rfc3339("2024-02-28T20:00:00.000Z", 540).as_deref(),
            Some("2024-02-29T05:00")
        );
        assert_eq!(from_rfc3339("not a date", 540), None);
    }

    /// What the editor shows, sent straight back, is the same instant.
    #[test]
    fn the_two_directions_agree() {
        let local = from_rfc3339("2026-08-31T20:00:00.000Z", 540).unwrap();
        assert_eq!(
            to_rfc3339(&local, 540).as_deref(),
            Some("2026-09-01T05:00:00+09:00")
        );
    }
}
