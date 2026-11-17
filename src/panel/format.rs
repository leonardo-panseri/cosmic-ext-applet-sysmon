// SPDX-License-Identifier: MPL-2.0

//! The text of panel values. Every function returns at most as many characters as its `WIDEST_*`
//! constant, which the panel button uses to size the value's cell (D6).

/// The widest text [`percent`] returns.
pub const WIDEST_PERCENT: &str = "100%";
/// The widest text [`celsius`] returns for a plausible temperature.
pub const WIDEST_CELSIUS: &str = "100°C";
/// The widest text [`rate`] returns.
pub const WIDEST_RATE: &str = "999K";

/// A share in percent, rounded to a whole number and clamped to 0–100: `"45%"`.
#[must_use]
pub fn percent(value: f32) -> String {
    format!("{:.0}%", value.clamp(0.0, 100.0))
}

/// A temperature, rounded to a whole degree: `"52°C"`.
#[must_use]
pub fn celsius(value: f32) -> String {
    format!("{value:.0}°C")
}

/// Bytes per second in at most four characters, in powers of 1024: `"999B"`, `"1.2K"`, `"340K"`,
/// `"12M"`. Negative or non-finite rates show as `"0B"`.
#[must_use]
pub fn rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 5] = ["K", "M", "G", "T", "P"];
    let bytes = if bytes_per_sec.is_finite() {
        bytes_per_sec.max(0.0)
    } else {
        0.0
    };
    if bytes < 999.5 {
        return format!("{bytes:.0}B");
    }
    let mut value = bytes / 1024.0;
    for unit in UNITS {
        // Switch unit where rounding would need a fourth digit.
        if value < 9.95 {
            return format!("{value:.1}{unit}");
        }
        if value < 999.5 {
            return format!("{value:.0}{unit}");
        }
        value /= 1024.0;
    }
    "999P".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(12.4), "12%");
        assert_eq!(percent(12.6), "13%");
        assert_eq!(percent(100.0), "100%");
        assert_eq!(percent(104.0), "100%");
        assert_eq!(percent(-3.0), "0%");
    }

    #[test]
    fn celsius_rounds_to_a_whole_degree() {
        assert_eq!(celsius(51.6), "52°C");
        assert_eq!(celsius(100.0), "100°C");
    }

    #[test]
    fn rate_uses_bytes_below_a_thousand() {
        assert_eq!(rate(0.0), "0B");
        assert_eq!(rate(999.0), "999B");
    }

    #[test]
    fn rate_keeps_one_decimal_below_ten_units() {
        assert_eq!(rate(1000.0), "1.0K");
        assert_eq!(rate(1229.0), "1.2K");
        assert_eq!(rate(9.94 * 1024.0), "9.9K");
    }

    #[test]
    fn rate_drops_the_decimal_from_ten_units() {
        assert_eq!(rate(9.96 * 1024.0), "10K");
        assert_eq!(rate(340.0 * 1024.0), "340K");
        assert_eq!(rate(999.4 * 1024.0), "999K");
    }

    #[test]
    fn rate_moves_to_the_next_unit_instead_of_a_fourth_digit() {
        assert_eq!(rate(999.6 * 1024.0), "1.0M");
        assert_eq!(rate(12.0 * 1024.0 * 1024.0), "12M");
        assert_eq!(rate(3.5 * 1024.0 * 1024.0 * 1024.0), "3.5G");
    }

    #[test]
    fn rate_never_exceeds_the_widest_text() {
        let mut bytes = 1.0;
        while bytes < 1e18 {
            for factor in [0.999, 1.0, 1.001, 9.95, 99.95, 999.5] {
                let text = rate(bytes * factor);
                assert!(
                    text.chars().count() <= WIDEST_RATE.chars().count(),
                    "{text}"
                );
            }
            bytes *= 10.0;
        }
    }

    #[test]
    fn rate_shows_nonsense_as_zero() {
        assert_eq!(rate(-5.0), "0B");
        assert_eq!(rate(f64::NAN), "0B");
    }
}
