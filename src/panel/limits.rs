// SPDX-License-Identifier: MPL-2.0

//! The warning limits (§2.1, §2.3): one per kind of warning, each with an on/off switch, stored
//! as one cosmic-config key each.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// A limit that is on or off. Stored as `(on: true, at: 85)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    /// Whether values at or above `at` warn.
    pub on: bool,
    /// °C or %, by the kind of limit.
    pub at: u8,
}

impl Limit {
    /// A limit that is on, at `at`.
    #[must_use]
    pub const fn on(at: u8) -> Self {
        Self { on: true, at }
    }

    /// A limit that is off, at `at` once turned on.
    #[must_use]
    pub const fn off(at: u8) -> Self {
        Self { on: false, at }
    }

    /// `at` once the limit is on, else `None`.
    #[must_use]
    pub fn active(self) -> Option<u8> {
        self.on.then_some(self.at)
    }
}

/// The `NVMe` limit: by default each drive's own rated limit (hwmon `temp*_crit`/`temp*_max`),
/// or one temperature for every drive. Stored as `(on: true, at: None)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvmeLimit {
    pub on: bool,
    /// The override, °C; `None` for each drive's rated limit.
    pub at: Option<u8>,
}

impl Default for NvmeLimit {
    fn default() -> Self {
        Self { on: true, at: None }
    }
}

/// What a limit is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Celsius,
    Percent,
}

/// Each kind of warning (§2.1), in the settings page's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKind {
    /// The CPU package temperature.
    CpuTemp,
    /// Any GPU's temperature.
    GpuTemp,
    /// Memory in use.
    Memory,
    /// Swap in use.
    Swap,
    /// Any filesystem's used space.
    Disk,
    /// Any GPU's VRAM in use.
    Vram,
    /// Any `NVMe` drive's temperature ([`NvmeLimit`]).
    Nvme,
}

impl LimitKind {
    pub const ALL: [Self; 7] = [
        Self::CpuTemp,
        Self::GpuTemp,
        Self::Memory,
        Self::Swap,
        Self::Disk,
        Self::Vram,
        Self::Nvme,
    ];

    #[must_use]
    pub fn unit(self) -> Unit {
        match self {
            Self::CpuTemp | Self::GpuTemp | Self::Nvme => Unit::Celsius,
            Self::Memory | Self::Swap | Self::Disk | Self::Vram => Unit::Percent,
        }
    }

    /// The limits the settings page accepts; others, typed or written by hand, are clamped.
    #[must_use]
    pub fn range(self) -> RangeInclusive<u8> {
        match self {
            Self::CpuTemp | Self::GpuTemp => 50..=110,
            Self::Nvme => 40..=100,
            // Swap in use is a sign of memory pressure well before it fills.
            Self::Swap => 10..=100,
            Self::Memory | Self::Disk | Self::Vram => 50..=100,
        }
    }

    /// `value` inside [`Self::range`].
    #[must_use]
    pub fn clamp(self, value: u8) -> u8 {
        let range = self.range();
        value.clamp(*range.start(), *range.end())
    }
}

/// Where the `NVMe` limit's − and + start from while it is each drive's rated limit: a typical
/// consumer drive's rated warning temperature.
pub const NVME_STEP_FROM: u8 = 80;
