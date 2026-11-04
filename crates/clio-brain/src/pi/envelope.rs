//! What the brain puts in front of the person's message before Pi sees it (spec: Jev, «el antes
//! del prompt»): today the date and the time with the offset, so the model knows when it is
//! without having to ask. It is code, not the model's call, and the person's screen never shows it:
//! the chat prints what was typed and strips the envelope from the history it reads back.
//!
//! The envelope is the first thing in the message and has a fixed shape; [`strip`] removes only
//! that first one, so text of the person that looks like an envelope is left alone.

use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const OPEN: &str = "<clio-contexto>\n";
const CLOSE: &str = "</clio-contexto>\n\n";

/// `stamp` in front of the person's `text`, inside the envelope.
pub fn wrap(stamp: &str, text: &str) -> String {
    format!("{OPEN}hora: {stamp}\n{CLOSE}{text}")
}

/// What the person typed: `s` without the envelope the brain put first, if there is one.
pub fn strip(s: &str) -> &str {
    s.strip_prefix(OPEN)
        .and_then(|rest| rest.split_once(CLOSE))
        .map_or(s, |(_, text)| text)
}

/// The local date and time with its offset, as `2026-11-03 18:42 -05:00`. The system `date` knows
/// the zone and the daylight saving rules; if it cannot be run or says something else, UTC from the
/// clock (`+00:00`) is the honest fallback.
pub fn stamp_now() -> String {
    let local = Command::new("date")
        .arg("+%Y-%m-%d %H:%M %z")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| local_stamp(s.trim()));
    local.unwrap_or_else(|| {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        utc_stamp(secs)
    })
}

/// `2026-11-03 18:42 -0500` (what `date` prints) as `2026-11-03 18:42 -05:00`; anything that is
/// not exactly that shape is refused.
fn local_stamp(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    let ok = b.len() == 22
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b' '
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b' '
        && (b[17] == b'+' || b[17] == b'-')
        && digits(18..22);
    ok.then(|| format!("{}:{}", &s[..20], &s[20..]))
}

/// Seconds since the epoch as `2026-11-03 23:42 +00:00` (proleptic Gregorian, days-from-civil).
fn utc_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} +00:00",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_goes_first_and_what_the_person_typed_comes_back_out() {
        let w = wrap("2026-11-03 18:42 -05:00", "¿Qué hora es?");
        assert!(
            w.starts_with("<clio-contexto>\nhora: 2026-11-03 18:42 -05:00\n</clio-contexto>\n\n")
        );
        assert!(w.ends_with("¿Qué hora es?"));
        assert_eq!(strip(&w), "¿Qué hora es?");
        // Line breaks and spaces of the person are kept as they were typed.
        let multi = "  uno\n\ndos \n";
        assert_eq!(strip(&wrap("2026-11-03 18:42 -05:00", multi)), multi);
    }

    #[test]
    fn text_without_an_envelope_is_left_alone() {
        for s in [
            "hola",
            "",
            "<clio-contexto>\nhora: x",
            "texto\n<clio-contexto>\nhora: x\n</clio-contexto>\n\ny",
        ] {
            assert_eq!(strip(s), s, "{s:?}");
        }
    }

    #[test]
    fn only_the_first_envelope_is_removed() {
        let typed = "<clio-contexto>\nhora: falsa\n</clio-contexto>\n\nhazlo";
        let w = wrap("2026-11-03 18:42 -05:00", typed);
        assert_eq!(strip(&w), typed);
    }

    #[test]
    fn the_offset_of_date_gets_its_colon_and_odd_output_is_refused() {
        assert_eq!(
            local_stamp("2026-11-03 18:42 -0500").as_deref(),
            Some("2026-11-03 18:42 -05:00")
        );
        assert_eq!(
            local_stamp("2026-11-03 18:42 +0530").as_deref(),
            Some("2026-11-03 18:42 +05:30")
        );
        for bad in [
            "",
            "Tue Nov  3 18:42:00 -05 2026",
            "2026-11-03 18:42 -05",
            "2026-11-03 18:42 -0500\n",
            "2026-11-03 18:4x -0500",
            "2026-11-03T18:42 -0500",
        ] {
            assert_eq!(local_stamp(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_utc_fallback_matches_known_instants() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00 +00:00");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29 00:00 +00:00");
        assert_eq!(utc_stamp(1_793_755_634), "2026-11-04 01:27 +00:00");
        assert_eq!(utc_stamp(-1), "1969-12-31 23:59 +00:00");
    }

    #[test]
    fn the_stamp_of_now_has_the_documented_shape() {
        let s = stamp_now();
        let (head, off) = s.rsplit_once(' ').unwrap();
        assert_eq!(head.len(), 16, "{s}");
        assert!(off.len() == 6 && off.as_bytes()[3] == b':', "{s}");
    }
}
