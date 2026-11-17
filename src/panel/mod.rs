// SPDX-License-Identifier: MPL-2.0

//! What the panel button shows (plan §2.1), as plain data: its settings, the formatted values
//! and the active warnings. The sampler builds a [`PanelSummary`] every tick and sends it only
//! when it differs from the last one (D6); the UI only lays it out.

pub mod format;

use std::ops::RangeInclusive;

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

use crate::sampler::stats::Stats;

/// How the panel button lays out its rows (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// Two small rows in one panel slot, with the network rates beside them.
    Stacked,
    /// Both values and the network rates on one line.
    OneLine,
    /// Only the top value. Vertical panels always use it.
    Compact,
}

impl Layout {
    /// Every layout, in the settings page's order.
    pub const ALL: [Self; 3] = [Self::Stacked, Self::OneLine, Self::Compact];
}

/// What a panel row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowKind {
    /// CPU load, %.
    Cpu,
    /// Memory in use, %.
    Memory,
    /// The first awake GPU's load, %.
    Gpu,
    /// The root filesystem's used space, %, as in the reference.
    Disk,
    /// CPU package temperature, °C.
    CpuTemp,
}

impl RowKind {
    /// Every row kind, in the settings page's order.
    pub const ALL: [Self; 5] = [
        Self::Cpu,
        Self::Memory,
        Self::Gpu,
        Self::Disk,
        Self::CpuTemp,
    ];

    /// The widest text this row's value can have, which sizes its cell (D6).
    #[must_use]
    pub fn widest_value(self) -> &'static str {
        match self {
            Self::CpuTemp => format::WIDEST_CELSIUS,
            Self::Cpu | Self::Memory | Self::Gpu | Self::Disk => format::WIDEST_PERCENT,
        }
    }
}

/// The temperatures the settings page offers as `warn_temp`, °C.
pub const WARN_TEMP_RANGE: RangeInclusive<u8> = 50..=110;
/// The shares the settings page offers as `warn_usage`, %.
pub const WARN_USAGE_RANGE: RangeInclusive<u8> = 50..=100;

/// The panel button's settings (§2.3), stored with cosmic-config as one key per field in
/// `~/.config/cosmic/<app id>/v1/`. Changed on the popup's settings page.
#[derive(Debug, Clone, PartialEq, Eq, CosmicConfigEntry)]
#[version = 1]
pub struct PanelSettings {
    pub layout: Layout,
    pub top: RowKind,
    pub bottom: RowKind,
    /// Whether the network rates are shown.
    pub network: bool,
    /// CPU and GPU temperatures at or above this warn, °C.
    pub warn_temp: u8,
    /// Memory or a filesystem at or above this share warns, %.
    pub warn_usage: u8,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            layout: Layout::Stacked,
            top: RowKind::Cpu,
            bottom: RowKind::Memory,
            network: true,
            warn_temp: 85,
            warn_usage: 90,
        }
    }
}

impl PanelSettings {
    /// These settings with the limits inside the ranges the settings page offers, for values
    /// written to the config by hand.
    #[must_use]
    pub fn clamped(self) -> Self {
        let clamp =
            |value: u8, range: &RangeInclusive<u8>| value.clamp(*range.start(), *range.end());
        Self {
            warn_temp: clamp(self.warn_temp, &WARN_TEMP_RANGE),
            warn_usage: clamp(self.warn_usage, &WARN_USAGE_RANGE),
            ..self
        }
    }

    /// The layout actually used: vertical panels only fit [`Layout::Compact`].
    #[must_use]
    pub fn layout_for(&self, horizontal: bool) -> Layout {
        if horizontal {
            self.layout
        } else {
            Layout::Compact
        }
    }

    /// The rows `layout` shows, top first.
    #[must_use]
    pub fn shown_rows(&self, layout: Layout) -> Vec<RowKind> {
        match layout {
            Layout::Stacked | Layout::OneLine => vec![self.top, self.bottom],
            Layout::Compact => vec![self.top],
        }
    }

    /// Whether a CPU or GPU temperature warns.
    #[must_use]
    pub fn hot(&self, celsius: f32) -> bool {
        celsius >= f32::from(self.warn_temp)
    }

    /// Whether memory or a filesystem this full, %, warns.
    #[must_use]
    pub fn full(&self, percent: f32) -> bool {
        percent >= f32::from(self.warn_usage)
    }
}

/// A value at or above its limit (§2.1), with both rounded to whole units for the tooltip.
/// Rounded, so a value moving within a degree or a percent doesn't make a new summary (D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The CPU package, °C.
    CpuTemp { celsius: i32, limit: i32 },
    /// A GPU, by DRM card, with its model when known; °C.
    GpuTemp {
        card: String,
        name: Option<String>,
        celsius: i32,
        limit: i32,
    },
    /// Memory in use, %.
    Memory { percent: i32, limit: i32 },
    /// A filesystem, by mount point; % full.
    Filesystem {
        mount: String,
        percent: i32,
        limit: i32,
    },
    /// An `nvme` drive, by name, against its own rated limit; °C.
    Nvme {
        drive: String,
        celsius: i32,
        limit: i32,
    },
}

impl Warning {
    /// Whether a row showing `kind` shows the value this warning is about, so it takes the
    /// warning's colour. The CPU and GPU rows show load, which has no limit; the Disk row shows
    /// only the root filesystem.
    #[must_use]
    pub fn colours(&self, kind: RowKind) -> bool {
        match self {
            Self::CpuTemp { .. } => kind == RowKind::CpuTemp,
            Self::Memory { .. } => kind == RowKind::Memory,
            Self::Filesystem { mount, .. } => kind == RowKind::Disk && mount == "/",
            Self::GpuTemp { .. } | Self::Nvme { .. } => false,
        }
    }
}

/// `value` rounded to a whole number, as the panel and the tooltip show it.
fn whole(value: f32) -> i32 {
    // Temperatures and percentages, far inside i32.
    #[expect(clippy::cast_possible_truncation, reason = "see above")]
    let whole = value.round() as i32;
    whole
}

/// Every warning `stats` raises under `settings`' limits.
#[must_use]
pub fn warnings(stats: &Stats, settings: &PanelSettings) -> Vec<Warning> {
    let temp_limit = i32::from(settings.warn_temp);
    let usage_limit = i32::from(settings.warn_usage);
    let mut warnings = Vec::new();
    if let Some(celsius) = stats.cpu_temp.filter(|&c| settings.hot(c)) {
        warnings.push(Warning::CpuTemp {
            celsius: whole(celsius),
            limit: temp_limit,
        });
    }
    warnings.extend(stats.gpus.iter().filter_map(|g| {
        let celsius = g.celsius.filter(|&c| settings.hot(c))?;
        Some(Warning::GpuTemp {
            card: g.card.clone(),
            name: g.name.clone(),
            celsius: whole(celsius),
            limit: temp_limit,
        })
    }));
    if let Some(percent) = stats.memory_percent().filter(|&p| settings.full(p)) {
        warnings.push(Warning::Memory {
            percent: whole(percent),
            limit: usage_limit,
        });
    }
    warnings.extend(stats.mounts.iter().filter_map(|m| {
        let percent = m.used_percent().filter(|&p| settings.full(p))?;
        Some(Warning::Filesystem {
            mount: m.mount_point.clone(),
            percent: whole(percent),
            limit: usage_limit,
        })
    }));
    warnings.extend(stats.drives.iter().filter_map(|d| {
        let limit = d.limit.filter(|&limit| d.celsius >= limit)?;
        Some(Warning::Nvme {
            drive: d.name.clone(),
            celsius: whole(d.celsius),
            limit: whole(limit),
        })
    }));
    warnings
}

/// Everything the panel button shows, formatted. `None` values show as a dash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSummary {
    pub top: Option<String>,
    pub bottom: Option<String>,
    /// Network download and upload; both `None` when the rates are hidden.
    pub down: Option<String>,
    pub up: Option<String>,
    /// Also listed in the panel button's tooltip.
    pub warnings: Vec<Warning>,
}

impl PanelSummary {
    #[must_use]
    pub fn new(stats: &Stats, settings: &PanelSettings) -> Self {
        let (down, up) = if settings.network {
            let total = stats.net_total;
            (
                total.map(|t| format::rate(t.first)),
                total.map(|t| format::rate(t.second)),
            )
        } else {
            (None, None)
        };
        Self {
            top: row_value(settings.top, stats),
            bottom: row_value(settings.bottom, stats),
            down,
            up,
            warnings: warnings(stats, settings),
        }
    }

    /// Whether a row showing `kind` takes the warning colour: its own value is at or above its
    /// limit.
    #[must_use]
    pub fn row_warns(&self, kind: RowKind) -> bool {
        self.warnings.iter().any(|w| w.colours(kind))
    }

    /// Whether the whole button takes the warning colour: some warning is about a value none of
    /// the `shown` rows shows.
    #[must_use]
    pub fn button_warns(&self, shown: &[RowKind]) -> bool {
        self.warnings
            .iter()
            .any(|w| !shown.iter().any(|kind| w.colours(*kind)))
    }
}

fn row_value(kind: RowKind, stats: &Stats) -> Option<String> {
    match kind {
        RowKind::Cpu => stats.cpu.map(format::percent),
        RowKind::Memory => stats.memory_percent().map(format::percent),
        RowKind::Gpu => stats
            .first_awake_gpu()
            .and_then(|g| g.busy)
            .map(|b| format::percent(f32::from(b))),
        RowKind::Disk => stats.root_fs_percent().map(format::percent),
        RowKind::CpuTemp => stats.cpu_temp.map(format::celsius),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::rates::PairRate;
    use crate::sampler::stats::{DriveTemp, GpuSample};
    use sysmon_proto::Mount;

    /// A filesystem at `mount_point`, `used` % full (no reserved blocks).
    fn mount(mount_point: &str, used: u64) -> Mount {
        Mount {
            mount_point: mount_point.into(),
            device: "/dev/sda1".into(),
            fs_type: "ext4".into(),
            total: 100,
            used,
            available: 100 - used,
        }
    }
    use crate::sources::drm::RuntimeStatus;
    use crate::sources::meminfo::MemInfo;

    fn memory(percent: u64) -> MemInfo {
        MemInfo {
            mem_total: 100,
            mem_available: 100 - percent,
            ..MemInfo::default()
        }
    }

    fn amd_gpu(busy: u8, celsius: f32) -> GpuSample {
        GpuSample {
            card: "card1".into(),
            name: None,
            status: RuntimeStatus::Active,
            busy: Some(busy),
            vram_used: None,
            vram_total: None,
            celsius: Some(celsius),
        }
    }

    #[test]
    fn default_rows_show_cpu_and_memory_with_network_rates() {
        let stats = Stats {
            cpu: Some(12.4),
            mem: Some(memory(45)),
            net_total: Some(PairRate {
                first: 1229.0,
                second: 20.0,
            }),
            ..Stats::default()
        };
        let summary = PanelSummary::new(&stats, &PanelSettings::default());
        assert_eq!(
            summary,
            PanelSummary {
                top: Some("12%".into()),
                bottom: Some("45%".into()),
                down: Some("1.2K".into()),
                up: Some("20B".into()),
                warnings: vec![],
            }
        );
    }

    #[test]
    fn missing_values_and_hidden_rates_are_none() {
        let settings = PanelSettings {
            network: false,
            ..PanelSettings::default()
        };
        let summary = PanelSummary::new(&Stats::default(), &settings);
        assert_eq!((summary.top, summary.down, summary.up), (None, None, None));
    }

    #[test]
    fn every_row_kind_shows_its_value() {
        let stats = Stats {
            gpus: vec![amd_gpu(37, 50.0)],
            mounts: vec![mount("/boot", 10), mount("/", 75)],
            cpu_temp: Some(61.6),
            ..Stats::default()
        };
        let summary_for = |top| {
            PanelSummary::new(
                &stats,
                &PanelSettings {
                    top,
                    ..PanelSettings::default()
                },
            )
            .top
        };
        assert_eq!(summary_for(RowKind::Gpu).as_deref(), Some("37%"));
        assert_eq!(summary_for(RowKind::Disk).as_deref(), Some("75%"));
        assert_eq!(summary_for(RowKind::CpuTemp).as_deref(), Some("62°C"));
    }

    #[test]
    fn warnings_start_at_the_limits() {
        let stats = Stats {
            cpu_temp: Some(85.0),
            gpus: vec![amd_gpu(10, 84.9)],
            mem: Some(memory(90)),
            mounts: vec![mount("/", 89), mount("/mnt/storage", 90)],
            drives: vec![
                DriveTemp {
                    name: "Fast SSD".into(),
                    celsius: 70.0,
                    limit: Some(70.0),
                },
                DriveTemp {
                    name: "No limit".into(),
                    celsius: 99.0,
                    limit: None,
                },
            ],
            ..Stats::default()
        };
        assert_eq!(
            warnings(&stats, &PanelSettings::default()),
            [
                Warning::CpuTemp {
                    celsius: 85,
                    limit: 85
                },
                Warning::Memory {
                    percent: 90,
                    limit: 90
                },
                Warning::Filesystem {
                    mount: "/mnt/storage".into(),
                    percent: 90,
                    limit: 90
                },
                Warning::Nvme {
                    drive: "Fast SSD".into(),
                    celsius: 70,
                    limit: 70
                },
            ]
        );
    }

    #[test]
    fn a_gpu_warning_names_the_card_and_its_model() {
        let mut nvidia = amd_gpu(10, 91.4);
        nvidia.card = "card0".into();
        nvidia.name = Some("RTX 4060".into());
        let stats = Stats {
            gpus: vec![nvidia],
            ..Stats::default()
        };
        assert_eq!(
            warnings(&stats, &PanelSettings::default()),
            [Warning::GpuTemp {
                card: "card0".into(),
                name: Some("RTX 4060".into()),
                celsius: 91,
                limit: 85
            }]
        );
    }

    #[test]
    fn the_limits_come_from_the_settings() {
        let stats = Stats {
            cpu_temp: Some(72.0),
            mem: Some(memory(60)),
            ..Stats::default()
        };
        assert_eq!(warnings(&stats, &PanelSettings::default()), Vec::new());
        let strict = PanelSettings {
            warn_temp: 70,
            warn_usage: 60,
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &strict),
            [
                Warning::CpuTemp {
                    celsius: 72,
                    limit: 70
                },
                Warning::Memory {
                    percent: 60,
                    limit: 60
                },
            ]
        );
    }

    #[test]
    fn values_moving_within_a_degree_keep_the_same_summary() {
        let at = |celsius| {
            PanelSummary::new(
                &Stats {
                    cpu_temp: Some(celsius),
                    ..Stats::default()
                },
                &PanelSettings::default(),
            )
        };
        assert_eq!(at(90.1), at(90.4));
        assert_ne!(at(90.4), at(90.6));
    }

    /// The default rows: CPU load over memory.
    const DEFAULT_ROWS: [RowKind; 2] = [RowKind::Cpu, RowKind::Memory];

    fn summary(stats: &Stats) -> PanelSummary {
        PanelSummary::new(stats, &PanelSettings::default())
    }

    #[test]
    fn memory_colours_its_own_row() {
        let summary = summary(&Stats {
            mem: Some(memory(95)),
            ..Stats::default()
        });
        assert!(summary.row_warns(RowKind::Memory));
        assert!(!summary.row_warns(RowKind::Cpu));
        assert!(!summary.button_warns(&DEFAULT_ROWS));
        // Off the bar: the whole button.
        assert!(summary.button_warns(&[RowKind::Cpu, RowKind::Disk]));
        assert!(summary.button_warns(&[RowKind::Cpu]));
    }

    #[test]
    fn cpu_temperature_colours_only_the_temperature_row() {
        let summary = summary(&Stats {
            cpu_temp: Some(90.0),
            ..Stats::default()
        });
        // The CPU row shows load, which has no limit: the warning isn't on the bar.
        assert!(!summary.row_warns(RowKind::Cpu));
        assert!(summary.button_warns(&DEFAULT_ROWS));
        assert!(summary.row_warns(RowKind::CpuTemp));
        assert!(!summary.button_warns(&[RowKind::Cpu, RowKind::CpuTemp]));
    }

    #[test]
    fn only_the_root_filesystem_colours_the_disk_row() {
        let root = summary(&Stats {
            mounts: vec![mount("/", 95)],
            ..Stats::default()
        });
        assert!(root.row_warns(RowKind::Disk));
        assert!(!root.button_warns(&[RowKind::Cpu, RowKind::Disk]));
        assert!(root.button_warns(&DEFAULT_ROWS));

        let other = summary(&Stats {
            mounts: vec![mount("/", 40), mount("/mnt/storage", 97)],
            ..Stats::default()
        });
        assert!(!other.row_warns(RowKind::Disk));
        assert!(other.button_warns(&[RowKind::Cpu, RowKind::Disk]));
    }

    #[test]
    fn gpu_and_nvme_temperatures_are_never_on_the_bar() {
        let summary = summary(&Stats {
            gpus: vec![amd_gpu(20, 95.0)],
            drives: vec![DriveTemp {
                name: "Samsung SSD 980 PRO".into(),
                celsius: 74.0,
                limit: Some(70.0),
            }],
            ..Stats::default()
        });
        assert_eq!(summary.warnings.len(), 2);
        for kind in RowKind::ALL {
            assert!(!summary.row_warns(kind), "{kind:?}");
        }
        assert!(summary.button_warns(&[RowKind::Gpu, RowKind::Disk]));
    }

    #[test]
    fn no_warnings_colour_nothing() {
        let summary = summary(&Stats {
            cpu_temp: Some(50.0),
            mem: Some(memory(40)),
            ..Stats::default()
        });
        assert_eq!(summary.warnings, Vec::new());
        assert!(!summary.button_warns(&DEFAULT_ROWS));
        assert!(RowKind::ALL.iter().all(|&kind| !summary.row_warns(kind)));
    }

    #[test]
    fn hand_written_limits_are_clamped_to_the_settings_page_ranges() {
        let wild = PanelSettings {
            warn_temp: 200,
            warn_usage: 10,
            ..PanelSettings::default()
        };
        let clamped = wild.clamped();
        assert_eq!((clamped.warn_temp, clamped.warn_usage), (110, 50));
        assert_eq!(PanelSettings::default().clamped(), PanelSettings::default());
    }

    #[test]
    fn vertical_panels_show_only_the_top_row() {
        let settings = PanelSettings::default();
        assert_eq!(settings.layout_for(true), Layout::Stacked);
        let layout = settings.layout_for(false);
        assert_eq!(layout, Layout::Compact);
        assert_eq!(settings.shown_rows(layout), [RowKind::Cpu]);
    }
}
