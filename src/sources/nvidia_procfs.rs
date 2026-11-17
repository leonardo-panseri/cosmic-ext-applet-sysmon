// SPDX-License-Identifier: MPL-2.0

//! `/proc/driver/nvidia/gpus/<pci address>/{information,power}`: what the NVIDIA driver says about a
//! card without waking it (D1, D4).
//!
//! Both files are `Key: value` lines, with indented sub-keys under section headers; only top-level
//! keys are read.

use super::clip_name;

/// The useful part of `information`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Information {
    /// Marketing name, e.g. `NVIDIA GeForce RTX 4060 Laptop GPU`.
    pub model: String,
    /// `GPU-…`; keys the VRAM-size cache (D4).
    pub uuid: Option<String>,
    /// PCI address, e.g. `0000:01:00.0`.
    pub bus_location: Option<String>,
}

/// Whether the card's video memory is powered (`Video Memory:` in `power`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMemory {
    Active,
    /// The card is in D3cold with its memory off.
    Off,
    Other,
}

/// The useful part of `power`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Power {
    /// `Runtime D3 status:` as written, e.g. "Enabled (fine-grained)", "Disabled", "Not supported".
    pub runtime_d3: Option<String>,
    pub video_memory: Option<VideoMemory>,
}

impl Power {
    /// Whether the driver may put the card in runtime D3 at all.
    #[must_use]
    pub fn runtime_d3_enabled(&self) -> bool {
        self.runtime_d3
            .as_deref()
            .is_some_and(|s| s.starts_with("Enabled"))
    }
}

/// Top-level `key: value` pairs, trimmed, with empty values skipped.
fn top_level(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(_, v)| !v.is_empty())
}

/// Parses `information`. Returns `None` without a `Model`.
#[must_use]
pub fn parse_information(text: &str) -> Option<Information> {
    let mut model = None;
    let mut info = Information::default();
    for (key, value) in top_level(text) {
        match key {
            "Model" => model = Some(clip_name(value)),
            "GPU UUID" => info.uuid = Some(clip_name(value)),
            "Bus Location" => info.bus_location = Some(clip_name(value)),
            _ => {}
        }
    }
    info.model = model?;
    Some(info)
}

/// Parses `power`. Missing keys stay `None`.
#[must_use]
pub fn parse_power(text: &str) -> Power {
    let mut power = Power::default();
    for (key, value) in top_level(text) {
        match key {
            "Runtime D3 status" => power.runtime_d3 = Some(clip_name(value)),
            "Video Memory" => {
                power.video_memory = Some(match value {
                    "Active" => VideoMemory::Active,
                    "Off" => VideoMemory::Off,
                    _ => VideoMemory::Other,
                });
            }
            _ => {}
        }
    }
    power
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn parses_the_reference_laptops_rtx_4060_information() {
        let info = parse_information(&test_fixtures::read(
            "nvidia_procfs/0000:01:00.0/information",
        ))
        .unwrap();
        assert_eq!(
            info,
            Information {
                model: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                uuid: Some("GPU-6e9f6c15-581b-4f11-c568-db1698fe7502".into()),
                bus_location: Some("0000:01:00.0".into()),
            }
        );
    }

    #[test]
    fn parses_the_reference_laptops_power_state_with_video_memory_off() {
        let power = parse_power(&test_fixtures::read("nvidia_procfs/0000:01:00.0/power"));
        assert_eq!(
            power,
            Power {
                runtime_d3: Some("Enabled (fine-grained)".into()),
                video_memory: Some(VideoMemory::Off),
            }
        );
        assert!(power.runtime_d3_enabled());
    }

    #[test]
    fn nested_keys_do_not_shadow_top_level_ones() {
        // " Video Memory Off:" and " Status:" are indented sub-keys under section headers.
        let power = parse_power(
            "GPU Hardware Support:\n Video Memory Off:          Supported\nVideo Memory:               Active\n",
        );
        assert_eq!(power.video_memory, Some(VideoMemory::Active));
        let power = parse_power("Runtime D3 status:          Not supported\n");
        assert!(!power.runtime_d3_enabled());
    }

    #[test]
    fn garbage_is_none_or_empty() {
        assert_eq!(parse_information(""), None);
        assert_eq!(parse_information("Model:\t\t \n"), None);
        assert_eq!(parse_information("\u{0}\u{ff}:::\nnonsense"), None);
        let info = parse_information(&format!("Model: {}\n", "x".repeat(500))).unwrap();
        assert_eq!(info.model.len(), 64);
        assert_eq!((info.uuid, info.bus_location), (None, None));
        assert_eq!(parse_power("\u{0}garbage\n:\n"), Power::default());
        assert_eq!(
            parse_power("Video Memory: Sideways").video_memory,
            Some(VideoMemory::Other)
        );
    }
}
