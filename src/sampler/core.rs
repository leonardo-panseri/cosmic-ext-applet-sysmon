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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// The popup opened (open tier) or closed (closed tier).
    PopupOpen(bool),
    /// The machine went on battery or back on AC (`UPower` `OnBattery`).
    OnBattery(bool),
    /// The panel button's settings changed (§2.3).
    Settings(PanelSettings),
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

    /// Whether the popup is open: the open tier.
    #[must_use]
    pub fn popup_open(&self) -> bool {
        self.popup_open
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
    /// the history without waiting for a tick. New settings return the last sample's update
    /// redone under them, if that changes what the panel button shows, so it redraws at once.
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
            Control::Settings(settings) => {
                self.settings = settings;
                let (at, last) = self.last.take()?;
                let update = Update {
                    panel: PanelSummary::new(&last.stats, &self.settings),
                    stats: last.stats,
                };
                self.last = Some((at, update.clone()));
                (self.sent.as_ref() != Some(&update.panel)).then(|| self.send(update))
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
    use crate::panel::{Layout, RowKind};
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
    fn new_settings_redo_the_last_sample_at_once() {
        let mut core = Core::new(PanelSettings::default());
        // Nothing sampled yet: nothing to send.
        assert!(
            core.control(Control::Settings(PanelSettings::default()))
                .is_none()
        );
        let t0 = Instant::now();
        let mut sample = memory(45);
        sample.cpu_temp = Some(88.0);
        core.sample(sample, t0);

        let shows_temp = PanelSettings {
            bottom: RowKind::CpuTemp,
            ..PanelSettings::default()
        };
        let update = core.control(Control::Settings(shows_temp.clone())).unwrap();
        assert_eq!(update.panel.bottom.as_deref(), Some("88°C"));
        // The hot CPU moved onto the bar: its row warns, not the whole button.
        let shown = shows_temp.shown_rows(shows_temp.layout);
        assert!(update.panel.row_warns(RowKind::CpuTemp));
        assert!(!update.panel.button_warns(&shown));
        // The same settings again change nothing.
        assert!(core.control(Control::Settings(shows_temp)).is_none());
        // Later samples follow the new settings.
        let mut sample = memory(45);
        sample.cpu_temp = Some(89.0);
        let update = core.sample(sample, t0 + INTERVAL).unwrap();
        assert_eq!(update.panel.bottom.as_deref(), Some("89°C"));

        // Back to memory: the hot CPU is off the bar again, so the whole button warns.
        let defaults = PanelSettings::default();
        let update = core.control(Control::Settings(defaults.clone())).unwrap();
        assert_eq!(update.panel.bottom.as_deref(), Some("45%"));
        assert!(
            update
                .panel
                .button_warns(&defaults.shown_rows(defaults.layout))
        );
    }

    #[test]
    fn a_new_limit_raises_or_clears_a_warning_at_once() {
        let mut core = Core::new(PanelSettings::default());
        core.sample(memory(80), Instant::now());
        let strict = PanelSettings {
            warn_usage: 75,
            ..PanelSettings::default()
        };
        let update = core.control(Control::Settings(strict)).unwrap();
        assert!(update.panel.row_warns(RowKind::Memory));
        let update = core
            .control(Control::Settings(PanelSettings::default()))
            .unwrap();
        assert_eq!(update.panel.warnings, Vec::new());
    }

    #[test]
    fn a_layout_change_alone_sends_nothing() {
        // The UI lays out the same summary; only the top and bottom values are in it.
        let mut core = Core::new(PanelSettings::default());
        core.sample(memory(45), Instant::now());
        let one_line = PanelSettings {
            layout: Layout::OneLine,
            ..PanelSettings::default()
        };
        assert!(core.control(Control::Settings(one_line.clone())).is_none());
        let hidden_rates = PanelSettings {
            network: false,
            ..one_line
        };
        // The rates were already `None` without a second sample, so nothing changes either.
        assert!(core.control(Control::Settings(hidden_rates)).is_none());
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
