// SPDX-License-Identifier: MPL-2.0

//! Rates from cumulative counters, over the measured time between two samples.

use std::collections::HashMap;
use std::time::Duration;

/// How fast a counter grew from `prev` to `cur` over `elapsed`, per second.
///
/// `None` when no time passed or the counter went backwards: it was reset (an interface came back,
/// a driver reloaded) or wrapped, so this interval's growth is unknown.
#[must_use]
pub fn per_second(prev: u64, cur: u64, elapsed: Duration) -> Option<f64> {
    let delta = cur.checked_sub(prev)?;
    let secs = elapsed.as_secs_f64();
    // Counter deltas over a few seconds are far below 2^52, so the cast is exact.
    #[expect(clippy::cast_precision_loss, reason = "see above")]
    (secs > 0.0).then(|| delta as f64 / secs)
}

/// Two counters of one device (received/sent, read/written), and the rates between two samples.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PairRate {
    pub first: f64,
    pub second: f64,
}

/// Remembers the previous counters of a set of named devices and turns the next counters into rates.
#[derive(Debug, Default)]
pub struct PairCounters {
    prev: HashMap<String, (u64, u64)>,
}

impl PairCounters {
    /// Records `counters` and returns each device's rates since the previous call, in input
    /// order. A device seen for the first time, or whose counters went backwards, gets no rate
    /// this time. Devices that disappeared are forgotten.
    pub fn update<'a>(
        &mut self,
        counters: impl IntoIterator<Item = (&'a str, u64, u64)>,
        elapsed: Option<Duration>,
    ) -> Vec<(String, Option<PairRate>)> {
        let mut next = HashMap::new();
        let rates = counters
            .into_iter()
            .map(|(name, first, second)| {
                let rate = elapsed
                    .zip(self.prev.get(name))
                    .and_then(|(elapsed, prev)| {
                        Some(PairRate {
                            first: per_second(prev.0, first, elapsed)?,
                            second: per_second(prev.1, second, elapsed)?,
                        })
                    });
                next.insert(name.to_owned(), (first, second));
                (name.to_owned(), rate)
            })
            .collect();
        self.prev = next;
        rates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_S: Duration = Duration::from_secs(2);

    #[test]
    fn per_second_divides_by_the_measured_elapsed_time() {
        assert_eq!(per_second(1000, 5000, TWO_S), Some(2000.0));
        assert_eq!(
            per_second(0, 3000, Duration::from_millis(2500)),
            Some(1200.0)
        );
    }

    #[test]
    fn per_second_has_no_rate_after_a_reset_or_without_time() {
        assert_eq!(per_second(5000, 10, TWO_S), None);
        assert_eq!(per_second(10, 20, Duration::ZERO), None);
    }

    #[test]
    fn pair_counters_need_two_samples() {
        let mut counters = PairCounters::default();
        assert_eq!(
            counters.update([("eth0", 100, 50)], None),
            [("eth0".to_owned(), None)]
        );
        assert_eq!(
            counters.update([("eth0", 300, 70)], Some(TWO_S)),
            [(
                "eth0".to_owned(),
                Some(PairRate {
                    first: 100.0,
                    second: 10.0
                })
            )]
        );
    }

    #[test]
    fn pair_counters_skip_a_device_that_was_reset_or_is_new() {
        let mut counters = PairCounters::default();
        counters.update([("eth0", 1000, 1000)], None);
        let rates = counters.update([("eth0", 10, 2000), ("wlan0", 5, 5)], Some(TWO_S));
        assert_eq!(rates, [("eth0".into(), None), ("wlan0".into(), None)]);
        // After the reset, eth0 counts from its new values.
        let rates = counters.update([("eth0", 30, 2000)], Some(TWO_S));
        assert_eq!(
            rates,
            [(
                "eth0".into(),
                Some(PairRate {
                    first: 10.0,
                    second: 0.0
                })
            )]
        );
    }
}
