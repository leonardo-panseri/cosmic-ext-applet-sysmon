// SPDX-License-Identifier: MPL-2.0

//! `/sys/class/hwmon/hwmon*/`: temperature sensors of one chip.
//!
//! The sampler reads a chip's `name` and the files [`is_wanted_file`] accepts, and hands them to
//! [`parse_chip`] as (file name, contents) pairs. [`classify`] then says what the chip measures.
//!
//! **Before reading a chip's files, the sampler must check where its `device` link points**: a GPU
//! driver's hwmon (nouveau, amdgpu on a dGPU) can wake a runtime-suspended card (D3).

use super::clip_name;
use std::collections::BTreeMap;

/// Most temperature sensors kept per chip (D8). Real chips have a handful; coretemp on a big
/// server has one per core plus the package.
pub const MAX_SENSORS: usize = 32;

/// Readings outside this range, in °C, are firmware garbage and are dropped.
const PLAUSIBLE_CELSIUS: std::ops::RangeInclusive<f32> = -50.0..=200.0;

/// One `temp<index>_*` sensor.
#[derive(Debug, Clone, PartialEq)]
pub struct TempSensor {
    /// The `<index>` in `temp<index>_input`.
    pub index: u32,
    pub label: Option<String>,
    /// Current temperature, °C.
    pub input: f32,
    /// The chip's "high" threshold (`temp*_max`), °C. On an `nvme` chip, the drive's warning temperature.
    pub max: Option<f32>,
    /// The critical threshold (`temp*_crit`), °C.
    pub crit: Option<f32>,
}

/// A hwmon chip: its driver name and its temperature sensors, ordered by index.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub name: String,
    pub temps: Vec<TempSensor>,
}

/// What a chip's main temperature means, for the chips the applet cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChipTemp {
    /// The CPU package (k10temp/zenpower `Tdie` or `Tctl`, coretemp `Package id 0`), °C.
    CpuPackage(f32),
    /// An AMD GPU's `edge` sensor, °C.
    AmdGpu(f32),
    /// An `nvme` chip's `Composite` sensor and its rated limit (`max`, else `crit`), °C.
    Nvme { celsius: f32, limit: Option<f32> },
}

#[derive(Clone, Copy)]
enum Field {
    Input,
    Label,
    Max,
    Crit,
}

/// Splits `temp<index>_<field>` into its parts.
fn temp_file(name: &str) -> Option<(u32, Field)> {
    let (index, field) = name.strip_prefix("temp")?.split_once('_')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = match field {
        "input" => Field::Input,
        "label" => Field::Label,
        "max" => Field::Max,
        "crit" => Field::Crit,
        _ => return None,
    };
    Some((index.parse().ok()?, field))
}

/// Whether the sampler should read this file of a hwmon chip directory.
#[must_use]
pub fn is_wanted_file(name: &str) -> bool {
    name == "name" || temp_file(name).is_some()
}

/// Parses a millidegree value into °C, dropping anything implausible.
fn celsius(text: &str) -> Option<f32> {
    let millis: i32 = text.trim().parse().ok()?;
    // Millidegrees within ±2^31 convert exactly to f64; the f32 rounding is far below 0.001 °C.
    #[allow(clippy::cast_possible_truncation)]
    let c = (f64::from(millis) / 1000.0) as f32;
    PLAUSIBLE_CELSIUS.contains(&c).then_some(c)
}

#[derive(Default)]
struct Partial {
    label: Option<String>,
    input: Option<f32>,
    max: Option<f32>,
    crit: Option<f32>,
}

/// Parses one chip directory given as (file name, contents) pairs. Ignores unknown files and bad
/// values; drops sensors without a valid `input`. Returns `None` without a non-empty `name`.
#[must_use]
pub fn parse_chip<K: AsRef<str>, V: AsRef<str>>(
    files: impl IntoIterator<Item = (K, V)>,
) -> Option<Chip> {
    let mut name = None;
    let mut sensors: BTreeMap<u32, Partial> = BTreeMap::new();
    for (file, contents) in files {
        let (file, contents) = (file.as_ref(), contents.as_ref());
        if file == "name" {
            name = Some(clip_name(contents)).filter(|n| !n.is_empty());
            continue;
        }
        let Some((index, field)) = temp_file(file) else {
            continue;
        };
        let sensor = sensors.entry(index).or_default();
        match field {
            Field::Input => sensor.input = celsius(contents),
            Field::Label => sensor.label = Some(clip_name(contents)).filter(|l| !l.is_empty()),
            Field::Max => sensor.max = celsius(contents),
            Field::Crit => sensor.crit = celsius(contents),
        }
    }
    let temps = sensors
        .into_iter()
        .filter_map(|(index, p)| {
            Some(TempSensor {
                index,
                label: p.label,
                input: p.input?,
                max: p.max,
                crit: p.crit,
            })
        })
        .take(MAX_SENSORS)
        .collect();
    Some(Chip { name: name?, temps })
}

impl Chip {
    fn labelled(&self, label: &str) -> Option<&TempSensor> {
        self.temps
            .iter()
            .find(|t| t.label.as_deref() == Some(label))
    }

    /// The first sensor, if the driver labels none of them (older kernels).
    fn sole_unlabelled(&self) -> Option<&TempSensor> {
        if self.temps.iter().any(|t| t.label.is_some()) {
            return None;
        }
        self.temps.first()
    }
}

/// Says what `chip` measures and picks its main temperature. `None` for chips the applet doesn't
/// show (ACPI zones, DIMMs, batteries, a second CPU socket, …).
#[must_use]
pub fn classify(chip: &Chip) -> Option<ChipTemp> {
    match chip.name.as_str() {
        // Tctl carries a fan-control offset on some Ryzens; Tdie, when present, is the real one.
        "k10temp" | "zenpower" => chip
            .labelled("Tdie")
            .or_else(|| chip.labelled("Tctl"))
            .or_else(|| chip.sole_unlabelled())
            .map(|t| ChipTemp::CpuPackage(t.input)),
        "coretemp" => chip
            .labelled("Package id 0")
            .map(|t| ChipTemp::CpuPackage(t.input)),
        "amdgpu" => chip
            .labelled("edge")
            .or_else(|| chip.sole_unlabelled())
            .map(|t| ChipTemp::AmdGpu(t.input)),
        "nvme" => chip
            .labelled("Composite")
            .or_else(|| chip.sole_unlabelled())
            .map(|t| ChipTemp::Nvme {
                celsius: t.input,
                limit: t.max.or(t.crit),
            }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    fn chip(rel: &str) -> Chip {
        parse_chip(test_fixtures::files(&format!("hwmon/{rel}"))).unwrap()
    }

    #[test]
    fn parses_k10temp_tctl_from_the_reference_laptop() {
        let chip = chip("hwmon5_k10temp");
        assert_eq!(chip.name, "k10temp");
        assert_eq!(
            chip.temps,
            [TempSensor {
                index: 1,
                label: Some("Tctl".into()),
                input: 40.375,
                max: None,
                crit: None
            }]
        );
        assert_eq!(classify(&chip), Some(ChipTemp::CpuPackage(40.375)));
    }

    #[test]
    fn amdgpu_edge_is_the_amd_gpu_temperature() {
        assert_eq!(
            classify(&chip("hwmon12_amdgpu")),
            Some(ChipTemp::AmdGpu(37.0))
        );
    }

    #[test]
    fn nvme_composite_carries_its_rated_limit() {
        let chip = chip("hwmon4_nvme");
        assert_eq!(chip.temps.len(), 2);
        assert_eq!(
            classify(&chip),
            Some(ChipTemp::Nvme {
                celsius: 33.85,
                limit: Some(86.85)
            })
        );
        assert_eq!(
            classify(&self::chip("hwmon3_nvme")),
            Some(ChipTemp::Nvme {
                celsius: 32.85,
                limit: Some(83.85)
            })
        );
    }

    #[test]
    fn implausible_values_are_dropped() {
        // The reference laptop's Micron drive reports temp2_max = 65261.85 °C.
        let chip = chip("hwmon4_nvme");
        assert_eq!(chip.temps[1].label.as_deref(), Some("Sensor 1"));
        assert_eq!(chip.temps[1].max, None);
    }

    #[test]
    fn chips_that_are_not_cpu_gpu_or_nvme_are_unclassified() {
        assert_eq!(classify(&chip("hwmon1_acpitz")), None);
        assert_eq!(classify(&chip("hwmon6_spd5118")), None);
    }

    #[test]
    fn intel_coretemp_package_id_0_is_the_cpu_package() {
        let chip = chip("handwritten_coretemp");
        assert_eq!(chip.temps.len(), 3);
        assert_eq!(classify(&chip), Some(ChipTemp::CpuPackage(52.0)));
        // A second socket's chip is ignored, so only one package temperature is ever reported.
        let socket1 = parse_chip([
            ("name", "coretemp\n"),
            ("temp1_label", "Package id 1\n"),
            ("temp1_input", "60000\n"),
        ])
        .unwrap();
        assert_eq!(classify(&socket1), None);
    }

    #[test]
    fn zenpower_prefers_tdie_over_the_offset_tctl() {
        assert_eq!(
            classify(&chip("handwritten_zenpower")),
            Some(ChipTemp::CpuPackage(45.25))
        );
    }

    #[test]
    fn unlabelled_k10temp_falls_back_to_temp1() {
        assert_eq!(
            classify(&chip("handwritten_k10temp_unlabelled")),
            Some(ChipTemp::CpuPackage(61.125))
        );
    }

    #[test]
    fn only_temperature_files_are_wanted() {
        for name in [
            "name",
            "temp1_input",
            "temp12_label",
            "temp3_crit",
            "temp1_max",
        ] {
            assert!(is_wanted_file(name), "{name}");
        }
        for name in [
            "power1_average",
            "temp1_alarm",
            "temp_input",
            "tempx_input",
            "fan1_input",
            "uevent",
            "temp1_input_",
        ] {
            assert!(!is_wanted_file(name), "{name}");
        }
    }

    #[test]
    fn garbage_is_skipped_and_a_chip_needs_a_name() {
        assert_eq!(parse_chip([("temp1_input", "40000")]), None);
        assert_eq!(parse_chip([("name", "   \n")]), None);
        let chip = parse_chip([
            ("name", "k10temp\n"),
            ("temp1_input", "garbage"),
            ("temp2_input", "\u{0}\u{ff}"),
            ("temp3_label", "Tctl"),
            ("temp99999999999_input", "1000"),
            ("tempX_input", "1000"),
            ("fan1_input", "1000"),
            ("temp4_input", "-273150"),
        ])
        .unwrap();
        assert!(chip.temps.is_empty(), "{chip:?}");
        assert_eq!(classify(&chip), None);
    }

    #[test]
    fn a_chip_keeps_at_most_32_sensors_and_clips_names() {
        let files: Vec<_> = (1..=100)
            .map(|i| (format!("temp{i}_input"), "30000".to_string()))
            .collect();
        let mut files = files;
        files.push(("name".into(), "x".repeat(200)));
        let chip = parse_chip(files).unwrap();
        assert_eq!(chip.temps.len(), MAX_SENSORS);
        assert_eq!(chip.temps[0].index, 1);
        assert_eq!(chip.name.len(), 64);
    }
}
