// SPDX-License-Identifier: MPL-2.0

//! What the panel button shows (plan §2.1), as plain data: its settings, the formatted values
//! and the active warnings. The sampler builds a [`PanelSummary`] every tick and sends it only
//! when it differs from the last one (D6); the UI only lays it out.

pub mod format;

use crate::sampler::stats::Stats;

/// How the panel button lays out its rows (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Two small rows in one panel slot, with the network rates beside them.
    Stacked,
    /// Both values and the network rates on one line.
    #[allow(dead_code, reason = "chosen on the settings page (P5)")]
    OneLine,
    /// Only the top value. Vertical panels always use it.
    Compact,
}

/// What a panel row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    #[allow(dead_code, reason = "chosen on the settings page (P5)")]
    CpuTemp,
}

impl RowKind {
    /// The widest text this row's value can have, which sizes its cell (D6).
    #[must_use]
    pub fn widest_value(self) -> &'static str {
        match self {
            Self::CpuTemp => format::WIDEST_CELSIUS,
            Self::Cpu | Self::Memory | Self::Gpu | Self::Disk => format::WIDEST_PERCENT,
        }
    }
}

/// The panel button's settings. These are the defaults of §2.3; P5 loads them from cosmic-config.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSettings {
    pub layout: Layout,
    pub top: RowKind,
    pub bottom: RowKind,
    /// Whether the network rates are shown.
    pub network: bool,
    /// CPU and GPU temperatures at or above this warn, °C.
    pub warn_temp: f32,
    /// Memory or a filesystem at or above this share warns, %.
    pub warn_usage: f32,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            layout: Layout::Stacked,
            top: RowKind::Cpu,
            bottom: RowKind::Memory,
            network: true,
            warn_temp: 85.0,
            warn_usage: 90.0,
        }
    }
}

impl PanelSettings {
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
}

/// A value at or above its limit (§2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    CpuTemp,
    /// A GPU, by DRM card.
    GpuTemp {
        card: String,
    },
    Memory,
    /// A filesystem, by mount point.
    Filesystem {
        mount: String,
    },
    /// An `nvme` drive at or above its own rated limit, by name.
    Nvme {
        drive: String,
    },
}

impl Warning {
    /// Whether a row showing `kind` is about the same part, so it takes this warning's colour.
    #[must_use]
    pub fn colours(&self, kind: RowKind) -> bool {
        match self {
            Self::CpuTemp => matches!(kind, RowKind::Cpu | RowKind::CpuTemp),
            Self::GpuTemp { .. } => kind == RowKind::Gpu,
            Self::Memory => kind == RowKind::Memory,
            Self::Filesystem { .. } | Self::Nvme { .. } => kind == RowKind::Disk,
        }
    }
}

/// Every warning `stats` raises under `settings`' limits.
#[must_use]
pub fn warnings(stats: &Stats, settings: &PanelSettings) -> Vec<Warning> {
    let hot = |c: f32| c >= settings.warn_temp;
    let full = |p: f32| p >= settings.warn_usage;
    let mut warnings = Vec::new();
    if stats.cpu_temp.is_some_and(hot) {
        warnings.push(Warning::CpuTemp);
    }
    warnings.extend(
        stats
            .gpus
            .iter()
            .filter(|g| g.celsius.is_some_and(hot))
            .map(|g| Warning::GpuTemp {
                card: g.card.clone(),
            }),
    );
    if stats.memory_percent().is_some_and(full) {
        warnings.push(Warning::Memory);
    }
    warnings.extend(
        stats
            .mounts
            .iter()
            .filter(|m| m.used_percent().is_some_and(full))
            .map(|m| Warning::Filesystem {
                mount: m.mount_point.clone(),
            }),
    );
    warnings.extend(
        stats
            .drives
            .iter()
            .filter(|d| d.limit.is_some_and(|limit| d.celsius >= limit))
            .map(|d| Warning::Nvme {
                drive: d.name.clone(),
            }),
    );
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

    /// Whether a row showing `kind` takes the warning colour.
    #[must_use]
    pub fn row_warns(&self, kind: RowKind) -> bool {
        self.warnings.iter().any(|w| w.colours(kind))
    }

    /// Whether the whole button takes the warning colour: some warning is about a part none of
    /// the `shown` rows is about.
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
                Warning::CpuTemp,
                Warning::Memory,
                Warning::Filesystem {
                    mount: "/mnt/storage".into()
                },
                Warning::Nvme {
                    drive: "Fast SSD".into()
                },
            ]
        );
    }

    #[test]
    fn a_warning_colours_its_row_when_shown_and_the_button_otherwise() {
        let stats = Stats {
            cpu_temp: Some(90.0),
            ..Stats::default()
        };
        let summary = PanelSummary::new(&stats, &PanelSettings::default());
        assert!(summary.row_warns(RowKind::Cpu));
        assert!(!summary.row_warns(RowKind::Memory));
        assert!(!summary.button_warns(&[RowKind::Cpu, RowKind::Memory]));
        assert!(summary.button_warns(&[RowKind::Memory, RowKind::Disk]));
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
