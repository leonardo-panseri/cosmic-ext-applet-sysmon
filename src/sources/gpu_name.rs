// SPDX-License-Identifier: MPL-2.0

//! An AMD GPU's name, looked up once when the card is first identified: the driver's
//! `product_name`, else the `pci.ids` entry of its vendor and device ids, else a generic name.
//!
//! Only for `amdgpu` cards: an NVIDIA card's ids are never read (its model comes from procfs,
//! which doesn't wake it, D3).

use std::io::BufRead;

/// Where `pci.ids` may be: hwdata's path, then the older one. The Flatpak runtime has the first.
pub const PCI_IDS_PATHS: [&str; 2] = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];
/// AMD's PCI vendor id.
pub const AMD_VENDOR: u16 = 0x1002;
/// Most `pci.ids` lines read in one lookup (the whole file has about 43 000).
const MAX_LINES: usize = 100_000;
/// Longest `pci.ids` line read; longer ones are cut there.
const MAX_LINE_BYTES: u64 = 512;
/// VRAM at or below this is an integrated GPU's carve-out from system memory, bytes.
const MAX_CARVE_OUT: u64 = 2 << 30;

/// Parses a sysfs id file such as `device/vendor` (`0x1002`).
#[must_use]
pub fn parse_id(text: &str) -> Option<u16> {
    let hex = text.trim().strip_prefix("0x")?;
    u16::from_str_radix(hex, 16).ok()
}

/// The name of `vendor:device` in a `pci.ids` file, as written, e.g. `Rembrandt [Radeon 680M]`.
/// Reads only up to the end of the vendor's section, and at most [`MAX_LINES`] lines.
pub fn lookup(pci_ids: impl BufRead, vendor: u16, device: u16) -> Option<String> {
    let vendor_prefix = format!("{vendor:04x} ");
    let device_prefix = format!("\t{device:04x} ");
    let mut in_vendor = false;
    let mut lines = pci_ids;
    let mut line = Vec::new();
    for _ in 0..MAX_LINES {
        line.clear();
        let n = std::io::Read::take(&mut lines, MAX_LINE_BYTES)
            .read_until(b'\n', &mut line)
            .ok()?;
        if n == 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&line);
        if text.starts_with('#') || text.trim().is_empty() {
            continue;
        }
        if !text.starts_with('\t') {
            if in_vendor {
                // The next vendor, or the class list: the device isn't listed.
                return None;
            }
            in_vendor = text.starts_with(&vendor_prefix);
        } else if in_vendor && let Some(name) = text.strip_prefix(&device_prefix) {
            let name = super::clip_name(name);
            return (!name.is_empty()).then_some(name);
        }
    }
    None
}

/// A display name for an AMD GPU from its `pci.ids` name: the marketing name in brackets, if
/// any, with the brand, so `Rembrandt [Radeon 680M]` is `AMD Radeon 680M`.
#[must_use]
pub fn amd_display_name(pci_name: &str) -> String {
    let name = pci_name
        .rsplit_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map_or(pci_name, |(inside, _)| inside)
        .trim();
    if name.starts_with("AMD") {
        super::clip_name(name)
    } else {
        super::clip_name(&format!("AMD {name}"))
    }
}

/// A name for an AMD GPU that has neither a `product_name` nor a `pci.ids` entry: an integrated
/// one is the boot display (`boot_vga`) or has no more VRAM than a carve-out.
#[must_use]
pub fn amd_generic_name(boot_vga: bool, vram_total: Option<u64>) -> &'static str {
    if boot_vga || vram_total.is_some_and(|v| v <= MAX_CARVE_OUT) {
        "AMD Radeon Graphics"
    } else {
        "AMD GPU"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCI_IDS: &str = "\
# List of PCI IDs
1000  Broadcom / LSI
\t0001  53c810
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t1478  Navi 10 XL Upstream Port of PCI Express Switch
\t1681  Rembrandt [Radeon 680M]
\t\t1043 19dd  Some laptop
\t15bf  Phoenix1
\t73bf  Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]
1003  ULSI Systems
\t1681  Not this one
C 00  Unclassified device
";

    #[test]
    fn finds_the_device_within_its_vendor() {
        let find = |vendor, device| lookup(PCI_IDS.as_bytes(), vendor, device);
        assert_eq!(
            find(0x1002, 0x1681).as_deref(),
            Some("Rembrandt [Radeon 680M]")
        );
        assert_eq!(find(0x1002, 0x15bf).as_deref(), Some("Phoenix1"));
        assert_eq!(find(0x1003, 0x1681).as_deref(), Some("Not this one"));
        // Listed under another vendor only, or not at all.
        assert_eq!(find(0x1002, 0x0001), None);
        assert_eq!(find(0x1002, 0xffff), None);
        assert_eq!(find(0xabcd, 0x1681), None);
    }

    #[test]
    fn garbage_finds_nothing() {
        assert_eq!(lookup(&b"\x00\xff\n\t1681  x\n"[..], 0x1002, 0x1681), None);
        let long = format!("1002  AMD\n\t1681  {}\n", "x".repeat(5000));
        assert_eq!(
            lookup(long.as_bytes(), 0x1002, 0x1681).map(|n| n.len()),
            Some(64)
        );
        assert_eq!(lookup(&b""[..], 0x1002, 0x1681), None);
    }

    #[test]
    fn ids_are_hex_with_a_prefix() {
        assert_eq!(parse_id("0x1002\n"), Some(AMD_VENDOR));
        assert_eq!(parse_id("0x1681"), Some(0x1681));
        assert_eq!(parse_id("1002"), None);
        assert_eq!(parse_id("0x10020"), None);
    }

    #[test]
    fn display_names_use_the_marketing_name() {
        assert_eq!(
            amd_display_name("Rembrandt [Radeon 680M]"),
            "AMD Radeon 680M"
        );
        assert_eq!(
            amd_display_name("Rembrandt [Radeon 660M / 680M]"),
            "AMD Radeon 660M / 680M"
        );
        assert_eq!(amd_display_name("Phoenix1"), "AMD Phoenix1");
        assert_eq!(
            amd_display_name("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]"),
            "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M"
        );
        assert_eq!(amd_display_name("AMD Radeon 780M"), "AMD Radeon 780M");
    }

    #[test]
    fn generic_names_tell_integrated_gpus_apart() {
        assert_eq!(amd_generic_name(true, None), "AMD Radeon Graphics");
        assert_eq!(
            amd_generic_name(false, Some(512 << 20)),
            "AMD Radeon Graphics"
        );
        assert_eq!(amd_generic_name(false, Some(16 << 30)), "AMD GPU");
        assert_eq!(amd_generic_name(false, None), "AMD GPU");
    }

    #[test]
    fn this_machine_s_pci_ids_if_any() {
        // Whatever the system has: the lookup must finish and stay bounded.
        for path in PCI_IDS_PATHS {
            if let Ok(file) = std::fs::File::open(path) {
                let name = lookup(std::io::BufReader::new(file), AMD_VENDOR, 0x1681);
                assert!(name.is_none_or(|n| n.chars().count() <= 64));
            }
        }
    }
}
