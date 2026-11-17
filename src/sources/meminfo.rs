// SPDX-License-Identifier: MPL-2.0

//! `/proc/meminfo`: memory and swap, in bytes.

/// Memory and swap sizes, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub mem_total: u64,
    /// What the kernel estimates can be given to programs without swapping.
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl MemInfo {
    /// Memory in use: total minus available.
    #[must_use]
    pub fn mem_used(&self) -> u64 {
        self.mem_total.saturating_sub(self.mem_available)
    }

    /// Swap in use: total minus free.
    #[must_use]
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

/// Parses `/proc/meminfo`. Returns `None` without `MemTotal` and `MemAvailable` (Linux ≥ 3.14);
/// swap is zero when its lines are missing.
#[must_use]
pub fn parse(text: &str) -> Option<MemInfo> {
    let (mut mem_total, mut mem_available) = (None, None);
    let mut mem = MemInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // Every size line is in kB (KiB, really); the unit is fixed by the kernel.
        let Some(bytes) = value
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kib| kib.saturating_mul(1024))
        else {
            continue;
        };
        match key {
            "MemTotal" => mem_total = Some(bytes),
            "MemAvailable" => mem_available = Some(bytes),
            "SwapTotal" => mem.swap_total = bytes,
            "SwapFree" => mem.swap_free = bytes,
            _ => {}
        }
    }
    mem.mem_total = mem_total?;
    mem.mem_available = mem_available?;
    Some(mem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn parses_memory_and_swap_from_the_reference_laptop_in_bytes() {
        let mem = parse(&test_fixtures::read("meminfo/meminfo")).unwrap();
        assert_eq!(mem.mem_total, 65_051_496 * 1024);
        assert_eq!(mem.mem_available, 55_805_696 * 1024);
        assert_eq!(mem.swap_total, 65_050_620 * 1024);
        assert_eq!(mem.swap_free, 65_049_488 * 1024);
        assert_eq!(mem.mem_used(), (65_051_496 - 55_805_696) * 1024);
        assert_eq!(mem.swap_used(), (65_050_620 - 65_049_488) * 1024);
    }

    #[test]
    fn a_machine_without_swap_has_zero_swap() {
        let mem = parse("MemTotal: 100 kB\nMemAvailable: 40 kB\n").unwrap();
        assert_eq!((mem.swap_total, mem.swap_used()), (0, 0));
    }

    #[test]
    fn used_never_goes_negative() {
        let mem =
            parse("MemTotal: 10 kB\nMemAvailable: 40 kB\nSwapTotal: 1 kB\nSwapFree: 5 kB").unwrap();
        assert_eq!((mem.mem_used(), mem.swap_used()), (0, 0));
    }

    #[test]
    fn garbage_or_missing_totals_are_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("MemTotal: lots kB\nMemAvailable: 4 kB"), None);
        assert_eq!(parse("MemTotal: 10 kB\n"), None);
        assert_eq!(parse("\u{0}:::\nMemTotal:\n:"), None);
        let huge = parse("MemTotal: 18446744073709551615 kB\nMemAvailable: 1 kB").unwrap();
        assert_eq!(huge.mem_total, u64::MAX);
    }
}
