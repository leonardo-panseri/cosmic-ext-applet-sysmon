// SPDX-License-Identifier: MPL-2.0

//! The sampler's decisions, without I/O: when to sample next (D2) and which samples to send to
//! the UI (D6). The thread in [`super`] feeds it samples and control messages.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::stats::{Sample, Stats, Tracker};
use crate::panel::{PanelSettings, PanelSummary};

/// The sampling interval, in both tiers (D2).
pub const INTERVAL: Duration = Duration::from_secs(2);
/// The closed tier's interval on battery (D2).
pub const BATTERY_CLOSED_INTERVAL: Duration = Duration::from_secs(4);

/// What the UI tells the sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// The popup opened (open tier) or closed (closed tier).
    PopupOpen(bool),
    /// The machine went on battery or back on AC (`UPower` `OnBattery`).
    OnBattery(bool),
}

/// What the sampler tells the UI.
#[derive(Debug, Clone)]
pub struct Update {
    pub panel: PanelSummary,
    /// The whole sample, with history, for the popup.
    pub stats: Arc<Stats>,
}

/// The sampler's state between ticks.
#[derive(Debug)]
pub struct Core {
    settings: PanelSettings,
    tracker: Tracker,
    popup_open: bool,
    on_battery: bool,
    /// When the last sample was taken, and its update.
    last: Option<(Instant, Update)>,
    /// The panel summary the UI has.
    sent: Option<PanelSummary>,
}

impl Core {
    #[must_use]
    pub fn new(settings: PanelSettings) -> Self {
        Self {
            settings,
            tracker: Tracker::default(),
            popup_open: false,
            on_battery: false,
            last: None,
            sent: None,
        }
    }

    /// The time between samples in the current tier.
    #[must_use]
    pub fn interval(&self) -> Duration {
        if self.on_battery && !self.popup_open {
            BATTERY_CLOSED_INTERVAL
        } else {
            INTERVAL
        }
    }

    /// When the next sample is due: one interval after the last, or right away.
    #[must_use]
    pub fn deadline(&self, now: Instant) -> Instant {
        self.last
            .as_ref()
            .map_or(now, |(at, _)| *at + self.interval())
    }

    /// Takes in the `sample` taken at `at`. Returns the update to send: every sample while the
    /// popup is open, otherwise only one that changes what the panel button shows.
    pub fn sample(&mut self, sample: Sample, at: Instant) -> Option<Update> {
        let stats = self.tracker.ingest(sample, at);
        let update = Update {
            panel: PanelSummary::new(&stats, &self.settings),
            stats: Arc::new(stats),
        };
        self.last = Some((at, update.clone()));
        (self.popup_open || self.sent.as_ref() != Some(&update.panel)).then(|| self.send(update))
    }

    /// Applies `control`. Opening the popup returns the last update right away, so the popup has
    /// the history without waiting for a tick.
    pub fn control(&mut self, control: Control) -> Option<Update> {
        match control {
            Control::PopupOpen(open) => {
                let opened = open && !self.popup_open;
                self.popup_open = open;
                let last = self.last.as_ref().map(|(_, update)| update.clone());
                last.filter(|_| opened).map(|update| self.send(update))
            }
            Control::OnBattery(on_battery) => {
                self.on_battery = on_battery;
                None
            }
        }
    }

    fn send(&mut self, update: Update) -> Update {
        self.sent = Some(update.panel.clone());
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::meminfo::MemInfo;

    fn memory(percent: u64) -> Sample {
        Sample {
            mem: Some(MemInfo {
                mem_total: 100,
                mem_available: 100 - percent,
                ..MemInfo::default()
            }),
            ..Sample::default()
        }
    }

    fn bottom(update: Option<Update>) -> Option<String> {
        update.and_then(|u| u.panel.bottom)
    }

    #[test]
    fn closed_tier_sends_only_what_changes_the_panel() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        assert_eq!(bottom(core.sample(memory(45), t0)).as_deref(), Some("45%"));
        assert!(core.sample(memory(45), t0 + INTERVAL).is_none());
        assert_eq!(
            bottom(core.sample(memory(46), t0 + 2 * INTERVAL)).as_deref(),
            Some("46%")
        );
    }

    #[test]
    fn open_tier_sends_every_sample() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        core.sample(memory(45), t0);
        core.control(Control::PopupOpen(true));
        assert!(core.sample(memory(45), t0 + INTERVAL).is_some());
        assert!(core.sample(memory(45), t0 + 2 * INTERVAL).is_some());
    }

    #[test]
    fn opening_the_popup_sends_the_last_sample_with_its_history_at_once() {
        let mut core = Core::new(PanelSettings::default());
        assert!(core.control(Control::PopupOpen(true)).is_none());
        core.control(Control::PopupOpen(false));
        let t0 = Instant::now();
        core.sample(memory(45), t0);
        core.sample(memory(45), t0 + INTERVAL);
        let update = core.control(Control::PopupOpen(true)).unwrap();
        assert_eq!(update.stats.history.net_down.iter().len(), 1);
        // Already open: nothing new to send.
        assert!(core.control(Control::PopupOpen(true)).is_none());
        assert!(core.control(Control::PopupOpen(false)).is_none());
    }

    #[test]
    fn a_change_while_open_is_not_sent_again_after_closing() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        core.sample(memory(45), t0);
        core.control(Control::PopupOpen(true));
        core.sample(memory(50), t0 + INTERVAL);
        core.control(Control::PopupOpen(false));
        assert!(core.sample(memory(50), t0 + 2 * INTERVAL).is_none());
    }

    #[test]
    fn the_closed_tier_slows_down_on_battery() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        assert_eq!(core.deadline(t0), t0);
        core.sample(memory(45), t0);
        assert_eq!(core.deadline(t0), t0 + INTERVAL);
        core.control(Control::OnBattery(true));
        assert_eq!(core.deadline(t0), t0 + BATTERY_CLOSED_INTERVAL);
        core.control(Control::PopupOpen(true));
        assert_eq!(core.deadline(t0), t0 + INTERVAL);
        core.control(Control::PopupOpen(false));
        core.control(Control::OnBattery(false));
        assert_eq!(core.deadline(t0), t0 + INTERVAL);
    }
}
