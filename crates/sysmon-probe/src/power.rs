// SPDX-License-Identifier: MPL-2.0

//! A PCI device's runtime power state, from `power/runtime_status`, and the one rule for when a
//! GPU may be queried (D3). Shared by the probe (before NVML) and the applet (DRM cards).

/// A device's runtime power-management state, from `power/runtime_status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuntimeStatus {
    Active,
    Suspended,
    Suspending,
    Resuming,
    /// Runtime PM is disabled for the device, so it stays powered.
    Unsupported,
    /// `error`, or anything this version doesn't know.
    #[default]
    Unknown,
}

impl RuntimeStatus {
    /// Whether querying the device is safe: it is awake and will stay so without our help.
    /// Every other state is treated as suspended (D3).
    #[must_use]
    pub fn may_query(self) -> bool {
        matches!(self, Self::Active | Self::Unsupported)
    }
}

/// Parses `power/runtime_status`.
#[must_use]
pub fn parse_runtime_status(text: &str) -> RuntimeStatus {
    match text.trim() {
        "active" => RuntimeStatus::Active,
        "suspended" => RuntimeStatus::Suspended,
        "suspending" => RuntimeStatus::Suspending,
        "resuming" => RuntimeStatus::Resuming,
        "unsupported" => RuntimeStatus::Unsupported,
        _ => RuntimeStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_active_or_always_on_device_may_be_queried() {
        let may: Vec<_> = [
            "active",
            "unsupported",
            "suspended",
            "suspending",
            "resuming",
            "error",
            "",
        ]
        .map(|s| parse_runtime_status(s).may_query())
        .to_vec();
        assert_eq!(may, [true, true, false, false, false, false, false]);
        assert_eq!(parse_runtime_status("resuming\n"), RuntimeStatus::Resuming);
        assert_eq!(
            parse_runtime_status("suspending"),
            RuntimeStatus::Suspending
        );
    }

    #[test]
    fn garbage_is_unknown() {
        assert_eq!(
            parse_runtime_status("\u{0}\u{ff}ACTIVE"),
            RuntimeStatus::Unknown
        );
    }
}
