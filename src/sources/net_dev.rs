// SPDX-License-Identifier: MPL-2.0

//! `/proc/net/dev`: bytes received and sent per network interface.
//!
//! [`parse`] returns every interface; [`physical`] then applies D7 (only interfaces with a
//! `/sys/class/net/<if>/device` link, found by [`super::discover::names_with_device_link`]) and
//! D8 (at most [`MAX_INTERFACES`]).

use super::{MAX_INTERFACES, clip_name};
use std::collections::HashSet;
use std::hash::BuildHasher;

/// Cumulative byte counters of one interface since it appeared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InterfaceCounters {
    /// Clipped to [`super::MAX_NAME_CHARS`]; kernel names are at most 15 bytes anyway.
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Parses `/proc/net/dev`, in file order. Skips the two header lines and any malformed line.
#[must_use]
pub fn parse(text: &str) -> Vec<InterfaceCounters> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<InterfaceCounters> {
    let (name, counters) = line.split_once(':')?;
    let name = clip_name(name);
    if name.is_empty() {
        return None;
    }
    // Receive: bytes packets errs drop fifo frame compressed multicast; then Transmit: bytes ...
    let mut fields = counters.split_whitespace().map(str::parse::<u64>);
    let rx_bytes = fields.next()?.ok()?;
    let tx_bytes = fields.nth(7)?.ok()?;
    Some(InterfaceCounters {
        name,
        rx_bytes,
        tx_bytes,
    })
}

/// Keeps the interfaces named in `with_device` (those backed by hardware, D7), in their original
/// order, at most [`MAX_INTERFACES`] of them.
#[must_use]
pub fn physical<S: BuildHasher>(
    interfaces: Vec<InterfaceCounters>,
    with_device: &HashSet<String, S>,
) -> Vec<InterfaceCounters> {
    interfaces
        .into_iter()
        .filter(|i| with_device.contains(&i.name))
        .take(MAX_INTERFACES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{MAX_INTERFACES, test_fixtures};
    use std::collections::HashSet;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn parses_every_interface_on_the_reference_laptop() {
        let ifaces = parse(&test_fixtures::read("net_dev/dev"));
        let names: Vec<_> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["lo", "eno1", "wlp4s0"]);
        assert_eq!(
            ifaces[2],
            InterfaceCounters {
                name: "wlp4s0".into(),
                rx_bytes: 2_605_183_118,
                tx_bytes: 108_991_726,
            }
        );
    }

    #[test]
    fn physical_keeps_only_interfaces_with_a_device_link_in_file_order() {
        let ifaces = parse(&test_fixtures::read("net_dev/two_nics_and_virtual"));
        assert_eq!(ifaces.len(), 7);
        // What /sys/class/net/*/device would show on such a machine; the order of the set is irrelevant.
        let phys = physical(ifaces, &set(&["wlan0", "enp3s0"]));
        let names: Vec<_> = phys.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["enp3s0", "wlan0"]);
        assert_eq!(phys[0].rx_bytes, 9_876_543_210);
        assert_eq!(phys[0].tx_bytes, 123_456_789);
    }

    #[test]
    fn physical_keeps_at_most_16_interfaces() {
        let text: String = (0..40)
            .map(|i| format!("eth{i}: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n"))
            .collect::<Vec<_>>()
            .concat();
        let all: HashSet<String> = (0..40).map(|i| format!("eth{i}")).collect();
        let phys = physical(parse(&text), &all);
        assert_eq!(phys.len(), MAX_INTERFACES);
        assert_eq!(phys[0].name, "eth0");
    }

    #[test]
    fn an_old_kernel_line_without_a_space_after_the_colon_parses() {
        let ifaces = parse("eth0:123 1 0 0 0 0 0 0 456 1 0 0 0 0 0 0");
        assert_eq!((ifaces[0].rx_bytes, ifaces[0].tx_bytes), (123, 456));
    }

    #[test]
    fn garbage_lines_are_skipped_and_names_are_clipped() {
        assert_eq!(parse(""), Vec::<InterfaceCounters>::new());
        assert_eq!(
            parse("\u{0}\u{ff}\n: 1 2\nx: a b c\neth0: 1 2 3\n"),
            Vec::<InterfaceCounters>::new()
        );
        let long = "n".repeat(300);
        let ifaces = parse(&format!("{long}: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0"));
        assert_eq!(ifaces[0].name.len(), 64);
    }
}
