// SPDX-License-Identifier: MPL-2.0

//! The icons the panel button and the popup share: the theme's symbolic icons, and the two
//! COSMIC's icon theme lacks, drawn here.

use cosmic::widget::icon;

use crate::panel::RowKind;

/// A panel row's icon, as the popup shows the same values.
pub fn row(kind: RowKind) -> icon::Handle {
    match kind {
        RowKind::Cpu => icon::from_name("utilities-system-monitor-symbolic").handle(),
        RowKind::Memory => memory(),
        RowKind::Gpu => icon::from_name("video-display-symbolic").handle(),
        RowKind::Disk => icon::from_name("drive-harddisk-system-symbolic").handle(),
        RowKind::CpuTemp => temperature(),
    }
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
