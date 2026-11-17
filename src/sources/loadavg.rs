// SPDX-License-Identifier: MPL-2.0

//! `/proc/loadavg`: the 1, 5 and 15 minute load averages.

/// The run-queue load averaged over 1, 5 and 15 minutes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LoadAvg {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
}

/// Parses `/proc/loadavg`. Returns `None` unless the first three fields are finite, non-negative numbers.
#[must_use]
pub fn parse(text: &str) -> Option<LoadAvg> {
    let mut fields = text.split_whitespace();
    let mut next = || {
        fields
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    Some(LoadAvg {
        one: next()?,
        five: next()?,
        fifteen: next()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn parses_the_three_averages_from_the_reference_laptop() {
        let load = parse(&test_fixtures::read("loadavg/loadavg")).unwrap();
        assert_eq!(
            load,
            LoadAvg {
                one: 0.82,
                five: 3.82,
                fifteen: 2.98
            }
        );
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("0.1 0.2"), None);
        assert_eq!(parse("a b c"), None);
        assert_eq!(parse("NaN inf -1"), None);
        assert_eq!(parse("1.0 -0.5 1.0"), None);
    }
}
