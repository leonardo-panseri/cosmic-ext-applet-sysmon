// SPDX-License-Identifier: MPL-2.0

//! `/proc/uptime`: time since boot.

use std::time::Duration;

/// Parses `/proc/uptime` into the time since boot. Returns `None` unless the first field is a
/// finite, non-negative number of seconds.
#[must_use]
pub fn parse(text: &str) -> Option<Duration> {
    let secs = text.split_whitespace().next()?.parse::<f64>().ok()?;
    Duration::try_from_secs_f64(secs).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn parses_time_since_boot_from_the_reference_laptop() {
        let up = parse(&test_fixtures::read("uptime/uptime")).unwrap();
        assert_eq!(up, Duration::from_millis(13_689_190));
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("abc 1"), None);
        assert_eq!(parse("-5.0 1"), None);
        assert_eq!(parse("NaN 1"), None);
        assert_eq!(parse("inf 1"), None);
        assert_eq!(parse("1e400 1"), None);
    }
}
