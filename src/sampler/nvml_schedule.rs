// SPDX-License-Identifier: MPL-2.0

//! When to ask the probe for NVML stats (ADR 0003), without I/O.
//!
//! Every NVML call wakes a suspended card, and calls every 5 s or more often keep an idle card
//! awake (spike S1). So the probe is asked only while an NVIDIA card is awake and someone looks
//! (the open tier, or `gpu` on the panel button, D2): on every tick while the card is busy, then,
//! after [`IDLE_SAMPLES`] samples in a row at 0 %, once every [`IDLE_INTERVAL`], so an idle card
//! can suspend. The first sample with load brings back every tick.

use std::time::{Duration, Instant};

/// Samples in a row at 0 % after which the schedule backs off.
pub const IDLE_SAMPLES: u32 = 3;
/// The time between samples once backed off. S1: calls every 10 s let an idle card suspend.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(15);

/// What the sampler knows on a tick, to decide whether to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub now: Instant,
    /// Some NVIDIA card's `runtime_status` is `active` (or runtime PM is off for it).
    pub awake: bool,
    /// The popup is open or the panel button shows `gpu`.
    pub wanted: bool,
}

/// The back-off's state between ticks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NvmlSchedule {
    /// When the probe was last asked.
    last: Option<Instant>,
    /// Samples in a row that showed no load.
    idle: u32,
}

impl NvmlSchedule {
    /// Whether to ask the probe on this tick.
    #[must_use]
    pub fn due(&self, tick: Tick) -> bool {
        if !(tick.awake && tick.wanted) {
            return false;
        }
        match self.last {
            Some(at) if self.backed_off() => {
                tick.now.saturating_duration_since(at) >= IDLE_INTERVAL
            }
            _ => true,
        }
    }

    /// Takes in what a sample asked at `at` showed: the highest load among the cards, `None` if
    /// none was measured (NVML missing or failing, or the card suspended meanwhile), which counts
    /// as idle so a failing NVML is asked rarely too.
    pub fn record(&mut self, at: Instant, load: Option<u8>) {
        self.last = Some(at);
        if load.is_some_and(|l| l > 0) {
            self.idle = 0;
        } else {
            self.idle = self.idle.saturating_add(1);
        }
    }

    /// Whether it asks only every [`IDLE_INTERVAL`].
    #[must_use]
    pub fn backed_off(&self) -> bool {
        self.idle >= IDLE_SAMPLES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::core::INTERVAL;

    /// Runs the schedule on 2 s ticks for `ticks` ticks from `t0`, with the card's load on each
    /// tick given by `load` (`None`: the card is suspended). Returns the ticks it asked on.
    fn run(
        schedule: &mut NvmlSchedule,
        t0: Instant,
        ticks: u32,
        load: impl Fn(u32) -> Option<u8>,
    ) -> Vec<u32> {
        let mut asked = Vec::new();
        for i in 0..ticks {
            let now = t0 + INTERVAL * i;
            let tick = Tick {
                now,
                awake: load(i).is_some(),
                wanted: true,
            };
            if schedule.due(tick) {
                asked.push(i);
                schedule.record(now, load(i));
            }
        }
        asked
    }

    #[test]
    fn a_busy_card_is_asked_every_tick() {
        let mut schedule = NvmlSchedule::default();
        let asked = run(&mut schedule, Instant::now(), 10, |_| Some(40));
        assert_eq!(asked, (0..10).collect::<Vec<_>>());
        assert!(!schedule.backed_off());
    }

    #[test]
    fn three_idle_samples_back_off_to_every_15_s() {
        let mut schedule = NvmlSchedule::default();
        let asked = run(&mut schedule, Instant::now(), 30, |_| Some(0));
        // Ticks 0, 1, 2 at 0 %, then at least 15 s apart: every 8th 2 s tick (16 s).
        assert_eq!(asked, [0, 1, 2, 10, 18, 26]);
        assert!(schedule.backed_off());
    }

    #[test]
    fn load_brings_back_every_tick_at_once() {
        let mut schedule = NvmlSchedule::default();
        // Idle until tick 12, then busy: it is seen on the next backed-off sample (tick 18).
        let asked = run(&mut schedule, Instant::now(), 24, |i| {
            Some(if i < 12 { 0 } else { 30 })
        });
        assert_eq!(asked, [0, 1, 2, 10, 18, 19, 20, 21, 22, 23]);
        assert!(!schedule.backed_off());
    }

    #[test]
    fn a_load_that_ends_backs_off_after_three_idle_samples() {
        let mut schedule = NvmlSchedule::default();
        let asked = run(&mut schedule, Instant::now(), 16, |i| {
            Some(if i < 5 { 70 } else { 0 })
        });
        // Busy 0–4; idle at 5, 6, 7; then 15 s later.
        assert_eq!(asked, [0, 1, 2, 3, 4, 5, 6, 7, 15]);
    }

    #[test]
    fn idle_load_needs_three_in_a_row() {
        let mut schedule = NvmlSchedule::default();
        let t0 = Instant::now();
        let loads = [0, 0, 5, 0, 0, 5, 0, 0];
        let asked = run(&mut schedule, t0, 8, |i| Some(loads[i as usize]));
        assert_eq!(asked, (0..8).collect::<Vec<_>>());
        assert!(!schedule.backed_off());
    }

    #[test]
    fn a_suspended_card_is_never_asked() {
        let mut schedule = NvmlSchedule::default();
        let asked = run(&mut schedule, Instant::now(), 10, |_| None);
        assert_eq!(asked, Vec::<u32>::new());
        assert_eq!(schedule, NvmlSchedule::default());
    }

    #[test]
    fn a_card_waking_while_backed_off_is_asked_once_the_interval_is_over() {
        let mut schedule = NvmlSchedule::default();
        let t0 = Instant::now();
        // Idle, backs off, suspends at tick 4, wakes at tick 20 (something else woke it).
        let asked = run(&mut schedule, t0, 30, |i| {
            (!(4..20).contains(&i)).then_some(0)
        });
        // Asked as soon as it is awake again (more than 15 s since tick 2), then every 16 s.
        assert_eq!(asked, [0, 1, 2, 20, 28]);
    }

    #[test]
    fn nobody_looking_means_no_samples() {
        let schedule = NvmlSchedule::default();
        let tick = Tick {
            now: Instant::now(),
            awake: true,
            wanted: false,
        };
        assert!(!schedule.due(tick));
        assert!(schedule.due(Tick {
            wanted: true,
            ..tick
        }));
    }

    #[test]
    fn a_failed_sample_counts_as_idle() {
        let mut schedule = NvmlSchedule::default();
        let t0 = Instant::now();
        for i in 0..3 {
            schedule.record(t0 + INTERVAL * i, None);
        }
        assert!(schedule.backed_off());
        assert!(!schedule.due(Tick {
            now: t0 + INTERVAL * 3,
            awake: true,
            wanted: true,
        }));
    }
}
