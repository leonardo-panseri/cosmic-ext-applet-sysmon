// SPDX-License-Identifier: MPL-2.0

//! `/sys/class/nvme/nvme*/model`: the model name of a drive.

use super::clip_name;

/// Parses `model`: the kernel pads it with spaces to 40 bytes. Returns `None` when blank.
#[must_use]
pub fn parse_model(text: &str) -> Option<String> {
    Some(clip_name(text)).filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_fixtures;

    #[test]
    fn trims_the_space_padding_of_the_reference_laptops_drives() {
        assert_eq!(
            parse_model(&test_fixtures::read("nvme/nvme0/model")).as_deref(),
            Some("Micron_2400_MTFDKBA512QFM")
        );
        assert_eq!(
            parse_model(&test_fixtures::read("nvme/nvme1/model")).as_deref(),
            Some("WD_BLACK SN850 1TB")
        );
    }

    #[test]
    fn blank_is_none_and_long_is_clipped() {
        assert_eq!(parse_model(" \n"), None);
        assert_eq!(parse_model(""), None);
        assert_eq!(parse_model(&"m".repeat(200)).map(|m| m.len()), Some(64));
        assert_eq!(
            parse_model("\u{0}\u{ff}").map(|m| m.chars().count()),
            Some(2)
        );
    }
}
