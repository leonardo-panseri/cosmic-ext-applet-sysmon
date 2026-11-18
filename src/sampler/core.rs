// SPDX-License-Identifier: MPL-2.0

//! The sampler's decisions, without I/O: when to sample next (D2) and which samples to send to
//! the UI (D6). The thread in [`super`] feeds it samples and control messages.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::readers::{Nvml, Wants};
use super::stats::{Sample, Stats, Tracker};
use crate::panel::{GpuRef, Item, PanelHysteresis, PanelSettings, PanelSummary};

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
    /// Stay in the open tier whatever the popup does (`SYSMON_DEBUG_OPEN`).
    force_open: bool,
    on_battery: bool,
    /// When the last sample was taken, and its update.
    last: Option<(Instant, Update)>,
    /// The panel summary the UI has, and when its sample was taken.
    sent: PanelHysteresis,
}

impl Core {
    #[must_use]
    pub fn new(settings: PanelSettings) -> Self {
        Self {
            settings,
            tracker: Tracker::default(),
            popup_open: false,
            force_open: false,
            on_battery: false,
            last: None,
            sent: PanelHysteresis::default(),
        }
    }

    /// With `force_open`, a sampler that stays in the open tier, for debugging.
    #[must_use]
    pub fn forced_open(mut self, force_open: bool) -> Self {
        self.force_open = force_open;
        self.popup_open |= force_open;
        self
    }

    /// What the current tier reads beyond the closed tier: the processes while the popup is
    /// open; NVIDIA stats then too, or whenever a GPU temperature or VRAM warning is on, or the
    /// panel button shows a GPU item: for any card if its GPU is Auto, else for its own card (D2).
    /// NVML is still asked only while a wanted card is awake, with the back-off of ADR 0003
    /// ([`super::nvml_schedule`]).
    #[must_use]
    pub fn wants(&self) -> Wants {
        let mut any =
            self.popup_open || self.settings.warn_gpu_temp.on || self.settings.warn_vram.on;
        let mut cards = Vec::new();
        for gpu in self.settings.rows.items().filter_map(Item::gpu) {
            match gpu {
                GpuRef::Auto => any = true,
                GpuRef::Pci(pci) => cards.push(pci.clone()),
            }
        }
        let nvml = if any {
            Nvml::Any
        } else if cards.is_empty() {
            Nvml::Off
        } else {
            Nvml::Cards(cards)
        };
        Wants {
            processes: self.popup_open,
            nvml,
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
    /// popup is open, otherwise only one that moves what the panel button shows enough
    /// ([`PanelHysteresis::worth_sending`]).
    pub fn sample(&mut self, sample: Sample, at: Instant) -> Option<Update> {
        let stats = self.tracker.ingest(sample, at);
        let update = Update {
            panel: PanelSummary::new(&stats, &self.settings),
            stats: Arc::new(stats),
        };
        self.last = Some((at, update.clone()));
        (self.popup_open || self.sent.worth_sending(&update.panel, at))
            .then(|| self.send(update, at))
    }

    /// Applies `control`. Opening the popup returns the last update right away, so the popup has
    /// the history without waiting for a tick. New settings return the last sample's update
    /// redone under them, if that changes what the panel button shows, so
    /// it redraws at once.
    pub fn control(&mut self, control: Control) -> Option<Update> {
        match control {
            Control::PopupOpen(open) => {
                let open = open || self.force_open;
                let opened = open && !self.popup_open;
                self.popup_open = open;
                let last = self.last.clone();
                last.filter(|_| opened)
                    .map(|(at, update)| self.send(update, at))
            }
            Control::OnBattery(on_battery) => {
                self.on_battery = on_battery;
                None
            }
            Control::Settings(settings) => {
                self.settings = settings;
                self.redo()
            }
        }
    }

    /// The last sample's update under the current settings, if that changes what
    /// the panel button shows at all: no hysteresis for a change the user made.
    fn redo(&mut self) -> Option<Update> {
        let (at, last) = self.last.take()?;
        let update = Update {
            panel: PanelSummary::new(&last.stats, &self.settings),
            stats: last.stats,
        };
        self.last = Some((at, update.clone()));
        self.sent
            .differs(&update.panel)
            .then(|| self.send(update, at))
    }

    /// Records that `update`, of the sample taken at `at`, is sent.
    fn send(&mut self, update: Update, at: Instant) -> Update {
        self.sent.sent(update.panel.clone(), at);
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{Layout, Limit, NetRef, Row, Rows};
    use crate::sampler::nvml_schedule::{NvmlSchedule, Tick};
    use crate::sources::drm::RuntimeStatus;
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

    /// The RAM item's value: the first of the bottom row in the default rows.
    fn ram(update: Option<Update>) -> Option<String> {
        update.and_then(|u| u.panel.bottom.first().and_then(|i| i.value.clone()))
    }

    #[test]
    fn closed_tier_sends_only_what_moves_the_panel_enough() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        assert_eq!(ram(core.sample(memory(45), t0)).as_deref(), Some("45%"));
        assert!(core.sample(memory(45), t0 + INTERVAL).is_none());
        // One point is under the threshold: the panel keeps showing 45 %.
        assert!(core.sample(memory(46), t0 + 2 * INTERVAL).is_none());
        assert_eq!(
            ram(core.sample(memory(47), t0 + 3 * INTERVAL)).as_deref(),
            Some("47%")
        );
    }

    #[test]
    fn closed_tier_catches_up_with_a_held_value() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        core.sample(memory(45), t0);
        let mut at = t0;
        for _ in 0..14 {
            at += INTERVAL;
            assert!(core.sample(memory(46), at).is_none());
        }
        // 30 s after the last send.
        let update = core.sample(memory(46), at + INTERVAL);
        assert_eq!(ram(update).as_deref(), Some("46%"));
    }

    #[test]
    fn closed_tier_measures_from_the_value_last_sent() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        core.sample(memory(45), t0);
        assert!(core.sample(memory(46), t0 + INTERVAL).is_none());
        // 47 % is two points from the 45 % on the panel, not one from the 46 % sampled.
        assert!(core.sample(memory(47), t0 + 2 * INTERVAL).is_some());
    }

    #[test]
    fn closed_tier_sends_a_new_warning_under_the_threshold() {
        let mut core = Core::new(PanelSettings::default());
        let t0 = Instant::now();
        core.sample(memory(89), t0);
        let update = core.sample(memory(90), t0 + INTERVAL).unwrap();
        assert!(update.panel.bottom[0].warns);
        let update = core.sample(memory(89), t0 + 2 * INTERVAL).unwrap();
        assert!(!update.panel.bottom[0].warns);
    }

    #[test]
    fn values_no_item_shows_send_nothing() {
        let no_ram = PanelSettings {
            rows: Rows::new(vec![Item::Cpu], vec![]),
            ..PanelSettings::default()
        };
        let mut core = Core::new(no_ram);
        let t0 = Instant::now();
        assert!(core.sample(memory(45), t0).is_some());
        assert!(core.sample(memory(60), t0 + INTERVAL).is_none());
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
            rows: Rows::new(vec![Item::Ram], vec![Item::CpuTemp]),
            ..PanelSettings::default()
        };
        let update = core.control(Control::Settings(shows_temp.clone())).unwrap();
        assert_eq!(update.panel.bottom[0].value.as_deref(), Some("88°C"));
        // The hot CPU's temperature item warns, not the whole button.
        assert!(update.panel.bottom[0].warns);
        assert!(!update.panel.button_warns);
        // The same settings again change nothing.
        assert!(core.control(Control::Settings(shows_temp)).is_none());
        // Later samples follow the new settings.
        let mut sample = memory(45);
        sample.cpu_temp = Some(89.0);
        let update = core.sample(sample, t0 + INTERVAL).unwrap();
        assert_eq!(update.panel.bottom[0].value.as_deref(), Some("89°C"));

        // No CPU item on the panel button: the whole button warns.
        let no_cpu = PanelSettings {
            rows: Rows::new(vec![Item::root_usage()], vec![Item::Ram]),
            ..PanelSettings::default()
        };
        let update = core.control(Control::Settings(no_cpu)).unwrap();
        assert_eq!(update.panel.bottom[0].value.as_deref(), Some("45%"));
        assert!(update.panel.button_warns);
    }

    #[test]
    fn a_new_limit_raises_or_clears_a_warning_at_once() {
        let mut core = Core::new(PanelSettings::default());
        core.sample(memory(80), Instant::now());
        let strict = PanelSettings {
            warn_memory: Limit::on(75),
            ..PanelSettings::default()
        };
        let update = core.control(Control::Settings(strict)).unwrap();
        assert!(update.panel.bottom[0].warns);
        let update = core
            .control(Control::Settings(PanelSettings::default()))
            .unwrap();
        assert_eq!(update.panel.warnings, Vec::new());
    }

    #[test]
    fn a_layout_change_alone_sends_nothing() {
        // Stacked and one-line show the same values, only laid out differently.
        let mut core = Core::new(PanelSettings::default());
        core.sample(memory(45), Instant::now());
        let one_line = PanelSettings {
            layout: Layout::OneLine,
            ..PanelSettings::default()
        };
        assert!(core.control(Control::Settings(one_line)).is_none());
    }

    #[test]
    fn a_new_item_is_sent_at_once() {
        let mut core = Core::new(PanelSettings::default());
        core.sample(memory(45), Instant::now());
        let mut rows = Rows::default();
        rows.push(Row::Top, Item::Upload(NetRef::All));
        let update = core
            .control(Control::Settings(PanelSettings {
                rows,
                ..PanelSettings::default()
            }))
            .unwrap();
        assert_eq!(update.panel.top.len(), 3);
    }

    /// The default settings with both NVIDIA warnings off.
    fn no_nvidia_warnings() -> PanelSettings {
        PanelSettings {
            warn_gpu_temp: Limit::off(85),
            warn_vram: Limit::off(90),
            ..PanelSettings::default()
        }
    }

    /// The settings without NVIDIA warnings, with `item` added to the top row.
    fn showing(item: Item) -> PanelSettings {
        let mut settings = no_nvidia_warnings();
        settings.rows.push(Row::Top, item);
        settings
    }

    /// Whether the closed tier asks NVML on its first tick, with the card at `pci` `awake` or
    /// not.
    fn asks_nvml(core: &Core, pci: &str, awake: bool) -> bool {
        let gpu = crate::sampler::stats::GpuSample {
            pci: Some(pci.into()),
            nvidia: true,
            status: if awake {
                RuntimeStatus::Active
            } else {
                RuntimeStatus::Suspended
            },
            ..Default::default()
        };
        let wants = core.wants();
        NvmlSchedule::default().due(Tick {
            now: Instant::now(),
            awake: gpu.status.may_query() && wants.nvml.covers(&gpu),
            wanted: wants.nvml != Nvml::Off,
        })
    }

    const NVIDIA: &str = "0000:01:00.0";
    const AMD: &str = "0000:05:00.0";

    #[test]
    fn an_nvidia_warning_asks_nvml_with_the_popup_closed_only_while_awake() {
        // The defaults: no GPU item, the GPU temperature warning on.
        let core = Core::new(PanelSettings::default());
        assert_eq!(core.wants().nvml, Nvml::Any);
        assert!(asks_nvml(&core, NVIDIA, true));
        assert!(!asks_nvml(&core, NVIDIA, false));
        let vram_only = Core::new(PanelSettings {
            warn_vram: Limit::on(90),
            ..no_nvidia_warnings()
        });
        assert!(asks_nvml(&vram_only, NVIDIA, true));
        assert!(!asks_nvml(&vram_only, NVIDIA, false));
    }

    #[test]
    fn without_nvidia_warnings_or_a_gpu_item_nvml_isn_t_wanted() {
        let core = Core::new(no_nvidia_warnings());
        assert_eq!(core.wants(), Wants::default());
        assert!(!asks_nvml(&core, NVIDIA, true));
    }

    #[test]
    fn an_auto_gpu_item_wants_any_awake_card() {
        let core = Core::new(showing(Item::GpuTemp(GpuRef::Auto)));
        assert_eq!(core.wants().nvml, Nvml::Any);
        assert!(asks_nvml(&core, NVIDIA, true));
        assert!(!asks_nvml(&core, NVIDIA, false));
    }

    #[test]
    fn a_gpu_item_for_one_card_wants_only_that_card_and_only_awake() {
        let core = Core::new(showing(Item::Vram(GpuRef::Pci(NVIDIA.into()))));
        assert_eq!(core.wants().nvml, Nvml::Cards(vec![NVIDIA.into()]));
        assert!(asks_nvml(&core, NVIDIA, true));
        assert!(!asks_nvml(&core, NVIDIA, false));
        // An item for the iGPU wants no NVML for the NVIDIA card.
        let igpu = Core::new(showing(Item::GpuUsage(GpuRef::Pci(AMD.into()))));
        assert!(!asks_nvml(&igpu, NVIDIA, true));
    }

    #[test]
    fn turning_an_nvidia_warning_on_or_off_takes_effect_at_once() {
        let mut core = Core::new(no_nvidia_warnings());
        core.control(Control::Settings(PanelSettings {
            warn_gpu_temp: Limit::on(85),
            ..no_nvidia_warnings()
        }));
        assert_eq!(core.wants().nvml, Nvml::Any);
        core.control(Control::Settings(no_nvidia_warnings()));
        assert_eq!(core.wants().nvml, Nvml::Off);
    }

    #[test]
    fn nvml_is_wanted_while_open_or_with_a_gpu_item() {
        let mut core = Core::new(no_nvidia_warnings());
        assert_eq!(core.wants(), Wants::default());
        core.control(Control::PopupOpen(true));
        assert_eq!(
            core.wants(),
            Wants {
                processes: true,
                nvml: Nvml::Any
            }
        );
        core.control(Control::PopupOpen(false));
        core.control(Control::Settings(showing(Item::GpuUsage(GpuRef::Auto))));
        assert_eq!(
            core.wants(),
            Wants {
                processes: false,
                nvml: Nvml::Any
            }
        );
    }

    #[test]
    fn a_forced_open_tier_stays_open() {
        let mut core = Core::new(PanelSettings::default()).forced_open(true);
        assert!(core.wants().processes);
        core.control(Control::PopupOpen(false));
        assert_eq!(core.wants().nvml, Nvml::Any);
        assert_eq!(core.interval(), INTERVAL);
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
