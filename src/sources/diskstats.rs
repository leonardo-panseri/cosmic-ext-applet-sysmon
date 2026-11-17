// SPDX-License-Identifier: MPL-2.0

//! `/proc/diskstats`: sectors read and written per block device.
//!
//! [`parse`] returns every device, partitions included; [`whole_disks`] then applies D7 (only
//! `/sys/block` entries with a `device` link, found by
//! [`super::discover::names_with_device_link`], and never loop, ram, zram or dm-*) and D8 (at
//! most [`MAX_DRIVES`]).

use super::{MAX_DRIVES, clip_name};
use std::collections::HashSet;
use std::hash::BuildHasher;

/// `/proc/diskstats` always counts in 512-byte sectors, whatever the device's real sector size.
pub const SECTOR_BYTES: u64 = 512;

/// Cumulative I/O counters of one block device since boot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskCounters {
    pub name: String,
    pub sectors_read: u64,
    pub sectors_written: u64,
}

/// Parses `/proc/diskstats`, in file order. Skips malformed lines.
#[must_use]
pub fn parse(text: &str) -> Vec<DiskCounters> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<DiskCounters> {
    // major minor name reads merged sectors_read ms_reading writes merged sectors_written ...
    let mut fields = line.split_whitespace();
    let name = clip_name(fields.nth(2)?);
    let mut counters = fields.map(str::parse::<u64>);
    let sectors_read = counters.nth(2)?.ok()?;
    let sectors_written = counters.nth(3)?.ok()?;
    Some(DiskCounters {
        name,
        sectors_read,
        sectors_written,
    })
}

/// Whether `name` is a virtual block device that is never counted as a drive: loop, ram, zram, dm-*.
#[must_use]
pub fn is_virtual(name: &str) -> bool {
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    numbered("loop") || numbered("ram") || numbered("zram") || numbered("dm-")
}

/// Keeps the devices named in `with_device` (whole disks backed by hardware, D7) that are not
/// [virtual](is_virtual), in their original order, at most [`MAX_DRIVES`] of them.
#[must_use]
pub fn whole_disks<S: BuildHasher>(
    disks: Vec<DiskCounters>,
    with_device: &HashSet<String, S>,
) -> Vec<DiskCounters> {
    disks
        .into_iter()
        .filter(|d| !is_virtual(&d.name) && with_device.contains(&d.name))
        .take(MAX_DRIVES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{MAX_DRIVES, test_fixtures};
    use std::collections::HashSet;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_every_device_on_the_reference_laptop() {
        let disks = parse(&test_fixtures::read("diskstats/diskstats"));
        assert_eq!(disks.len(), 6);
        assert_eq!(
            disks[2],
            DiskCounters {
                name: "nvme0n1".into(),
                sectors_read: 14_522_220,
                sectors_written: 19_510_666,
            }
        );
    }

    #[test]
    fn whole_disks_on_the_reference_laptop_are_the_two_nvme_drives() {
        let disks = parse(&test_fixtures::read("diskstats/diskstats"));
        // /sys/block entries with a `device` link on the reference laptop (zram0 has none).
        let disks = whole_disks(disks, &set(&["nvme0n1", "nvme1n1"]));
        let names: Vec<_> = disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["nvme1n1", "nvme0n1"]);
    }

    #[test]
    fn whole_disks_drops_partitions_and_virtual_devices_even_with_a_device_link() {
        let disks = parse(&test_fixtures::read("diskstats/mixed"));
        // Pretend every name has a device link: the name rule alone must drop loop, ram, zram, dm-*.
        let all: HashSet<String> = disks.iter().map(|d| d.name.clone()).collect();
        let names: Vec<_> = whole_disks(disks.clone(), &all)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["sda", "sda1", "mmcblk0", "mmcblk0p1", "nvme0n1"]);
        // With the real /sys/block set, partitions go too.
        let names: Vec<_> =
            whole_disks(disks, &set(&["sda", "mmcblk0", "nvme0n1", "loop0", "dm-0"]))
                .into_iter()
                .map(|d| d.name)
                .collect();
        assert_eq!(names, ["sda", "mmcblk0", "nvme0n1"]);
    }

    #[test]
    fn virtual_device_names_are_recognised() {
        for name in ["loop0", "loop12", "ram0", "zram0", "dm-0", "dm-12"] {
            assert!(is_virtual(name), "{name}");
        }
        for name in ["sda", "nvme0n1", "mmcblk0", "vda", "dmz", "rampage"] {
            assert!(!is_virtual(name), "{name}");
        }
    }

    #[test]
    fn whole_disks_keeps_at_most_8_drives() {
        let text: String = (0..20)
            .map(|i| format!("8 {i} sd{i} 1 0 2 0 3 0 4 0 0 0 0\n"))
            .collect::<Vec<_>>()
            .concat();
        let all: HashSet<String> = (0..20).map(|i| format!("sd{i}")).collect();
        assert_eq!(whole_disks(parse(&text), &all).len(), MAX_DRIVES);
    }

    #[test]
    fn garbage_lines_are_skipped() {
        assert_eq!(parse(""), Vec::<DiskCounters>::new());
        assert_eq!(
            parse("\u{0}\u{ff}\n8 0 sda x y z\n8 0 sda 1 2\n   \n"),
            Vec::<DiskCounters>::new()
        );
        let disks = parse("8 0 sda 1 0 2 0 3 0 4 0 0 0 0\nnonsense\n");
        assert_eq!((disks[0].sectors_read, disks[0].sectors_written), (2, 4));
    }
}
