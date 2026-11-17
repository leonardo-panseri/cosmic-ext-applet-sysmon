// SPDX-License-Identifier: MPL-2.0

//! `/proc/stat`: CPU time per state, in jiffies, for the whole machine and for each core.

/// Cumulative time one CPU (or all of them) spent in each state since boot, in jiffies.
///
/// `guest` and `guest_nice` are left out: the kernel already counts them in `user` and `nice`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// All time accounted for.
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.user,
            self.nice,
            self.system,
            self.idle,
            self.iowait,
            self.irq,
            self.softirq,
            self.steal,
        ]
        .iter()
        .fold(0, |sum, v| sum.saturating_add(*v))
    }

    /// Time doing nothing: `idle` plus `iowait` (waiting on I/O is not work for the CPU).
    #[must_use]
    pub fn idle_total(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }

    fn from_fields<'a>(mut fields: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut next = || fields.next()?.parse::<u64>().ok();
        Some(Self {
            user: next()?,
            nice: next()?,
            system: next()?,
            idle: next()?,
            // Kernels before 2.6.33 stop here or earlier; anything missing counts as zero.
            iowait: next().unwrap_or(0),
            irq: next().unwrap_or(0),
            softirq: next().unwrap_or(0),
            steal: next().unwrap_or(0),
        })
    }
}

/// The CPU lines of `/proc/stat`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcStat {
    /// The `cpu` line: all cores together.
    pub total: CpuTimes,
    /// The `cpuN` lines in file order. Offline cores are missing from the file, so an index here
    /// is a position, not necessarily `N`.
    pub cores: Vec<CpuTimes>,
}

/// Parses `/proc/stat`. Returns `None` without a well-formed `cpu` line; skips malformed `cpuN` lines.
#[must_use]
pub fn parse(text: &str) -> Option<ProcStat> {
    let mut total = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(label) = fields.next() else { continue };
        let Some(id) = label.strip_prefix("cpu") else {
            continue;
        };
        if id.is_empty() {
            total = CpuTimes::from_fields(fields);
        } else if id.bytes().all(|b| b.is_ascii_digit())
            && let Some(times) = CpuTimes::from_fields(fields)
        {
            cores.push(times);
        }
    }
    Some(ProcStat {
        total: total?,
        cores,
    })
}

/// How busy a CPU was between two samples, in percent (0–100).
///
/// Returns `None` when no time passed or a counter went backwards (CPU hotplug, a bad read).
#[must_use]
pub fn busy_percent(prev: &CpuTimes, cur: &CpuTimes) -> Option<f32> {
    let total = cur.total().checked_sub(prev.total())?;
    let idle = cur.idle_total().checked_sub(prev.idle_total())?;
    if total == 0 || idle > total {
        return None;
    }
    // Jiffy deltas between two samples are far below 2^52, so the cast to f64 is exact.
    #[allow(clippy::cast_precision_loss)]
    let busy = (total - idle) as f64 / total as f64 * 100.0;
    #[allow(clippy::cast_possible_truncation)]
    Some(busy as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn parses_total_and_every_core_from_the_reference_laptop() {
        let stat = parse(&test_fixtures::read("proc_stat/stat_a")).unwrap();
        assert_eq!(
            stat.total,
            CpuTimes {
                user: 495_053,
                nice: 10_332,
                system: 95_879,
                idle: 19_330_253,
                iowait: 1_622_236,
                irq: 44_197,
                softirq: 13_308,
                steal: 0,
            }
        );
        assert_eq!(stat.cores.len(), 16);
        assert_eq!(stat.cores[1].user, 26_218);
        assert_eq!(stat.cores[1].softirq, 412);
    }

    #[test]
    fn busy_percent_between_two_real_samples_is_within_range() {
        let a = parse(&test_fixtures::read("proc_stat/stat_a")).unwrap();
        let b = parse(&test_fixtures::read("proc_stat/stat_b")).unwrap();
        let busy = busy_percent(&a.total, &b.total).unwrap();
        assert!((0.0..=100.0).contains(&busy), "{busy}");
        assert_eq!(a.cores.len(), b.cores.len());
    }

    #[test]
    fn busy_percent_counts_iowait_as_idle_and_steal_as_busy() {
        let prev = CpuTimes::default();
        let cur = CpuTimes {
            user: 10,
            system: 10,
            idle: 50,
            iowait: 10,
            steal: 20,
            ..CpuTimes::default()
        };
        assert_eq!(busy_percent(&prev, &cur), Some(40.0));
    }

    #[test]
    fn busy_percent_is_none_when_no_time_passed_or_counters_went_backwards() {
        let a = parse(&test_fixtures::read("proc_stat/stat_a")).unwrap();
        let b = parse(&test_fixtures::read("proc_stat/stat_b")).unwrap();
        assert_eq!(busy_percent(&a.total, &a.total), None);
        assert_eq!(busy_percent(&b.total, &a.total), None);
    }

    #[test]
    fn short_lines_from_old_kernels_count_missing_states_as_zero() {
        let stat = parse("cpu 1 2 3 4\ncpu0 1 2 3 4 5\n").unwrap();
        assert_eq!(stat.total.iowait, 0);
        assert_eq!(stat.cores[0].iowait, 5);
    }

    #[test]
    fn garbage_is_none_and_bad_core_lines_are_skipped() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("\u{0}\u{ff}garbage\ncpu x y z\n"), None);
        assert_eq!(parse("cpu 1 2 3"), None);
        let stat = parse("cpu 1 2 3 4\ncpu0 a b c d\ncpufoo 1 2 3 4\ncpu1 1 2 3 4\n").unwrap();
        assert_eq!(stat.cores.len(), 1);
        assert_eq!(
            parse("cpu 18446744073709551615 18446744073709551615 1 1").map(|s| s.total.total()),
            Some(u64::MAX)
        );
    }
}
