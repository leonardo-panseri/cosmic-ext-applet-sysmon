// SPDX-License-Identifier: MPL-2.0

//! `/sys/class/drm/card*/device/`: AMD GPU load and VRAM, and every card's runtime power state.
//!
//! Read `power/runtime_status` first and touch nothing else of a card unless
//! [`RuntimeStatus::may_query`] says so (D3).

pub use sysmon_probe::power::{RuntimeStatus, parse_runtime_status};

/// Parses amdgpu's `gpu_busy_percent` (0–100).
#[must_use]
pub fn parse_busy_percent(text: &str) -> Option<u8> {
    text.trim().parse::<u8>().ok().filter(|p| *p <= 100)
}

/// Parses a plain byte count such as amdgpu's `mem_info_vram_used` and `mem_info_vram_total`.
#[must_use]
pub fn parse_bytes(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn the_reference_laptops_cards_are_one_active_and_one_suspended() {
        let amd = parse_runtime_status(&test_fixtures::read(
            "drm/card1_amdgpu/power/runtime_status",
        ));
        let nvidia = parse_runtime_status(&test_fixtures::read(
            "drm/card0_nvidia/power/runtime_status",
        ));
        assert_eq!(
            (amd, nvidia),
            (RuntimeStatus::Active, RuntimeStatus::Suspended)
        );
    }

    #[test]
    fn parses_amd_load_and_vram_from_the_reference_laptop() {
        let busy = parse_busy_percent(&test_fixtures::read("drm/card1_amdgpu/gpu_busy_percent"));
        let used = parse_bytes(&test_fixtures::read("drm/card1_amdgpu/mem_info_vram_used"));
        let total = parse_bytes(&test_fixtures::read("drm/card1_amdgpu/mem_info_vram_total"));
        assert_eq!(
            (busy, used, total),
            (Some(0), Some(363_872_256), Some(536_870_912))
        );
        assert_eq!(parse_busy_percent("100\n"), Some(100));
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse_busy_percent("101"), None);
        assert_eq!(parse_busy_percent("-1"), None);
        assert_eq!(parse_busy_percent(""), None);
        assert_eq!(parse_bytes("12 MB"), None);
        assert_eq!(parse_bytes("\u{0}"), None);
        assert_eq!(parse_bytes("99999999999999999999999"), None);
    }
}
