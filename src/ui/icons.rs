// SPDX-License-Identifier: MPL-2.0

//! The icons the panel button and the popup share: the theme's symbolic icons, and the three
//! COSMIC's icon theme lacks, drawn here.

use cosmic::widget::icon;

use crate::fl;
use crate::panel::Item;

/// What stands before an item's value: an icon, or for a network rate an arrow, as the
/// reference shows them.
pub enum Glyph {
    Icon(icon::Handle),
    Text(String),
}

/// An item's glyph, as the popup shows the same values: a device's icon, a thermometer for the
/// CPU temperature, ↓ and ↑ for the network rates. A GPU's or a drive's temperature takes its
/// device's icon, so the panel tells the three temperatures apart by icon, and each from its
/// device's other values by unit.
pub fn item(item: &Item) -> Glyph {
    let named = |name: &str| Glyph::Icon(icon::from_name(name).handle());
    match item {
        Item::Cpu => Glyph::Icon(cpu()),
        Item::CpuTemp => Glyph::Icon(temperature()),
        Item::Ram | Item::Vram(_) => Glyph::Icon(memory()),
        Item::Swap => named("media-flash-symbolic"),
        Item::GpuUsage(_) | Item::GpuTemp(_) => named("video-display-symbolic"),
        Item::DiskUsage(_) => named("drive-harddisk-system-symbolic"),
        Item::DiskRead(_) => named("document-open-symbolic"),
        Item::DiskWrite(_) => named("document-save-symbolic"),
        Item::NvmeTemp(_) => named("drive-harddisk-solidstate-symbolic"),
        Item::Download(_) => Glyph::Text(fl!("download")),
        Item::Upload(_) => Glyph::Text(fl!("upload")),
    }
}

/// The CPU: COSMIC's icon theme and its fallbacks (Pop, hicolor, Adwaita) have no chip icon.
pub fn cpu() -> icon::Handle {
    embedded(include_bytes!("../../resources/icons/cpu-symbolic.svg"))
}

/// Memory and VRAM: COSMIC's icon theme has no memory icon.
pub fn memory() -> icon::Handle {
    embedded(include_bytes!("../../resources/icons/memory-symbolic.svg"))
}

/// Temperatures: COSMIC's icon theme has no thermometer.
pub fn temperature() -> icon::Handle {
    embedded(include_bytes!(
        "../../resources/icons/temperature-symbolic.svg"
    ))
}

fn embedded(svg: &'static [u8]) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(svg);
    // Drawn in the text colour, like the theme's symbolic icons.
    handle.symbolic = true;
    handle
}
