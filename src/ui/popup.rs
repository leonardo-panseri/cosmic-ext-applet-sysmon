// SPDX-License-Identifier: MPL-2.0

//! The popup (plan §2.2, §2.3): the stats page in a scrollable area over a footer with the
//! **Process monitor** and **Settings** buttons, or the settings page.
//!
//! Everything here lays out the latest [`Stats`] and reads nothing else, so a redraw costs only
//! the layout. All system-supplied text (models, interface names, mount points, process names) is
//! plain text, clipped to one line (D8).

use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, icon};

use crate::app::Message;
use crate::fl;
use crate::panel::{LimitKind, PanelSettings, format};
use crate::sampler::stats::{GpuSample, Stats, share};
use crate::ui::settings::{self, SettingsModels};
use crate::ui::{graph, icons};
use sysmon_proto::Process;

/// The stats page scrolls beyond this, so the popup stays on screen with the footer.
pub const PAGE_MAX_HEIGHT: f32 = 680.0;
/// The width of the value column of a stat row: "212G / 928G".
const VALUE_WIDTH: f32 = 104.0;
/// The width of the percentage column: "100%".
const PERCENT_WIDTH: f32 = 44.0;
const ICON_SIZE: u16 = 16;
/// A core at or above this load, %, shows in the warning colour, as in the reference.
const BUSY_CORE: f32 = 85.0;
/// The opacity of secondary text, relative to the text colour.
const MUTED_ALPHA: f32 = 0.65;

/// Which page the popup shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Stats,
    /// The panel button's settings (§2.3).
    Settings,
}

/// The popup's content: `page`, for `stats` (`None` before the first sample), with values at or
/// above `settings`' limits in the warning colour. The stats page scrolls beyond `max_height`;
/// the settings page shows `settings` with the segmented controls of `models`.
pub fn view<'a>(
    stats: Option<&'a Stats>,
    settings: &PanelSettings,
    models: &'a SettingsModels,
    page: Page,
    max_height: f32,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let content: Element<'a, Message> = match page {
        Page::Stats => {
            let body = match stats {
                Some(stats) => stats_page(stats, settings),
                None => widget::text::body(fl!("placeholder")).into(),
            };
            let body = widget::container(body).padding([0, spacing.space_s]);
            widget::Column::new()
                .push(widget::container(widget::scrollable(body)).max_height(max_height))
                .push(widget::container(footer()).padding([0, spacing.space_s]))
                .spacing(spacing.space_s)
                .into()
        }
        // Padded inside, around its scrollable area.
        Page::Settings => settings::view(models, settings),
    };
    widget::container(content)
        .padding([spacing.space_s, 0])
        .width(Length::Fill)
        .into()
}

/// The sections of §2.2, in the reference's order, between dividers.
fn stats_page<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    with_dividers(sections(stats, settings))
}

fn sections<'a>(stats: &'a Stats, settings: &PanelSettings) -> Vec<Element<'a, Message>> {
    [
        Some(cpu(stats, settings)),
        Some(memory(stats, settings)),
        (!stats.gpus.is_empty()).then(|| gpus(stats, settings)),
        Some(network(stats)),
        Some(disk(stats, settings)),
        Some(processes(stats)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn with_dividers(sections: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    let mut page = widget::Column::new().spacing(cosmic::theme::spacing().space_xs);
    for (i, section) in sections.into_iter().enumerate() {
        if i > 0 {
            page = page.push(widget::divider::horizontal::default());
        }
        page = page.push(section);
    }
    page.into()
}

/// The whole stats page in two halves, the first `at` sections and the rest with the footer,
/// padded as in the popup, to see all of it at once without scrolling (the preview).
#[cfg(debug_assertions)]
pub fn stats_page_halves<'a>(
    stats: &'a Stats,
    settings: &PanelSettings,
    at: usize,
) -> [Element<'a, Message>; 2] {
    let spacing = cosmic::theme::spacing();
    let mut first = sections(stats, settings);
    let second = first.split_off(at.min(first.len()));
    let pad = |content: Element<'a, Message>| -> Element<'a, Message> {
        widget::container(content)
            .padding([spacing.space_s, spacing.space_s])
            .width(Length::Fill)
            .into()
    };
    [
        pad(with_dividers(first)),
        pad(widget::Column::new()
            .spacing(spacing.space_s)
            .push(with_dividers(second))
            .push(footer())
            .into()),
    ]
}

/// Big total load and uptime on the left; thread count, load average and package temperature on
/// the right; then the graph and the per-core bars.
fn cpu<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let left = widget::Column::new()
        .push(widget::text::title2(
            stats.cpu.map_or_else(|| fl!("no-value"), format::percent),
        ))
        .push(muted(widget::text::caption(
            stats.uptime.map_or_else(String::new, uptime),
        )));
    let threads = i64::try_from(stats.cores.len()).unwrap_or(i64::MAX);
    let mut right = widget::Column::new()
        .align_x(Alignment::End)
        .push(widget::text::body(fl!("cpu-threads", threads = threads)));
    if let Some(load) = stats.load {
        right = right.push(muted(widget::text::caption(format!(
            "{:.2} {:.2} {:.2}",
            load.one, load.five, load.fifteen
        ))));
    }
    if let Some(temp) = stats.cpu_temp {
        right = right.push(warn_if(
            widget::text::body(format::celsius(temp)),
            settings.warns(LimitKind::CpuTemp, temp),
        ));
    }
    widget::Column::new()
        .spacing(spacing.space_xxs)
        .push(
            widget::Row::new()
                .align_y(Alignment::Center)
                .push(left)
                .push(widget::Space::new().width(Length::Fill))
                .push(right),
        )
        .push(graph::percent(&stats.history.cpu))
        .push(graph::core_bars(&stats.cores, BUSY_CORE))
        .into()
}

fn memory<'a>(stats: &Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let mut rows = Vec::new();
    if let Some(mem) = stats.mem {
        let percent = stats.memory_percent();
        rows.push(stat_row(
            memory_icon(),
            fl!("in-use"),
            used_of_total(mem.mem_used(), mem.mem_total),
            percent.map(|p| (format::percent(p), settings.warns(LimitKind::Memory, p))),
        ));
        let swap = share(mem.swap_used(), mem.swap_total).unwrap_or(0.0);
        rows.push(stat_row(
            named_icon("media-flash-symbolic"),
            fl!("swap"),
            used_of_total(mem.swap_used(), mem.swap_total),
            Some((format::percent(swap), settings.warns(LimitKind::Swap, swap))),
        ));
    }
    section(fl!("section-memory"), rows)
}

/// One block per GPU, the NVIDIA cards first. A suspended card shows `Suspended`, and its VRAM
/// as 0 of the size cached the last time it was awake, if ever (D4).
fn gpus<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let history = |card: &str| {
        stats
            .history
            .gpus
            .iter()
            .find(|(c, _)| c == card)
            .map(|(_, h)| h)
    };
    let mut blocks = Vec::new();
    for gpu in stats.gpus_nvidia_first() {
        blocks.push(gpu_header(gpu, settings));
        if gpu.busy.is_some()
            && let Some(history) = history(&gpu.card)
        {
            blocks.push(graph::percent(history));
        }
        if let (Some(used), Some(total)) = (gpu.vram_used, gpu.vram_total) {
            let percent = share(used, total).unwrap_or(0.0);
            blocks.push(stat_row(
                memory_icon(),
                fl!("vram"),
                used_of_total(used, total),
                Some((
                    format::percent(percent),
                    settings.warns(LimitKind::Vram, percent),
                )),
            ));
        }
    }
    section(fl!("section-gpu"), blocks)
}

/// The GPU's name, then "load · temperature · power", or "Suspended".
fn gpu_header<'a>(gpu: &GpuSample, settings: &PanelSettings) -> Element<'a, Message> {
    let name = gpu
        .name
        .clone()
        .unwrap_or_else(|| fl!("gpu-unnamed", card = gpu.card.as_str()));
    let mut values = widget::Row::new().align_y(Alignment::Center);
    if gpu.suspended() {
        values = values.push(muted(widget::text::body(fl!("gpu-suspended"))));
    } else {
        let mut parts: Vec<Element<'a, Message>> = Vec::new();
        if let Some(busy) = gpu.busy {
            parts.push(widget::text::body(format::percent(f32::from(busy))).into());
        }
        if let Some(temp) = gpu.celsius {
            parts.push(
                warn_if(
                    widget::text::body(format::celsius(temp)),
                    settings.warns(LimitKind::GpuTemp, temp),
                )
                .into(),
            );
        }
        if let Some(watts) = gpu.watts {
            parts.push(widget::text::body(format::watts(watts)).into());
        }
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                values = values.push(muted(widget::text::body(fl!("stat-separator"))));
            }
            values = values.push(part);
        }
    }
    widget::Row::new()
        .spacing(cosmic::theme::spacing().space_xxs)
        .align_y(Alignment::Center)
        .push(named_icon("video-display-symbolic"))
        .push(one_line(widget::text::body(name)).width(Length::Fill))
        .push(values)
        .into()
}

/// Download and upload with a mirrored graph; one block per interface when more than one moved
/// data in the last two minutes (D7), else the total.
fn network(stats: &Stats) -> Element<'_, Message> {
    let in_use: Vec<_> = stats
        .history
        .interfaces
        .iter()
        .filter(|h| h.had_traffic())
        .collect();
    let mut blocks = Vec::new();
    if in_use.len() > 1 {
        for history in in_use {
            let rate = stats
                .net
                .iter()
                .find(|d| d.name == history.name)
                .and_then(|d| d.rate);
            blocks.push(
                widget::Row::new()
                    .spacing(cosmic::theme::spacing().space_xxs)
                    .align_y(Alignment::Center)
                    .push(named_icon(interface_icon(&history.name)))
                    .push(one_line(widget::text::body(history.name.as_str())))
                    .into(),
            );
            blocks.extend(rates(
                rate.map(|r| (r.first, r.second)),
                [
                    ("network-receive-symbolic", fl!("download-label")),
                    ("network-transmit-symbolic", fl!("upload-label")),
                ],
            ));
            blocks.push(graph::mirrored(&history.down, &history.up));
        }
    } else {
        blocks.extend(rates(
            stats.net_total.map(|r| (r.first, r.second)),
            [
                ("network-receive-symbolic", fl!("download-label")),
                ("network-transmit-symbolic", fl!("upload-label")),
            ],
        ));
        blocks.push(graph::mirrored(
            &stats.history.net_down,
            &stats.history.net_up,
        ));
    }
    section(fl!("section-network"), blocks)
}

/// Read and write rates, space per filesystem and the `NVMe` temperatures.
fn disk<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let mut rows = rates(
        stats.disk_total.map(|r| (r.first, r.second)),
        [
            ("document-open-symbolic", fl!("read")),
            ("document-save-symbolic", fl!("write")),
        ],
    );
    for mount in &stats.mounts {
        let percent = mount.used_percent();
        rows.push(stat_row(
            named_icon("folder-symbolic"),
            mount.mount_point.clone(),
            used_of_total(mount.used, mount.total),
            percent.map(|p| (format::percent(p), settings.warns(LimitKind::Disk, p))),
        ));
    }
    for drive in &stats.drives {
        rows.push(
            widget::Row::new()
                .spacing(cosmic::theme::spacing().space_xxs)
                .align_y(Alignment::Center)
                .push(temperature_icon())
                .push(one_line(widget::text::body(drive.name.as_str())).width(Length::Fill))
                .push(warn_if(
                    widget::text::body(format::celsius(drive.celsius)),
                    settings.nvme_hot(drive),
                ))
                .into(),
        );
    }
    section(fl!("section-disk"), rows)
}

/// The top processes by CPU and by memory, side by side.
fn processes(stats: &Stats) -> Element<'_, Message> {
    let body: Element<'_, Message> = match &stats.processes {
        Some(top) => widget::Row::new()
            .spacing(cosmic::theme::spacing().space_m)
            .push(process_column(fl!("processes-cpu"), &top.by_cpu, |p| {
                format!("{:.0}%", p.cpu.max(0.0))
            }))
            .push(process_column(
                fl!("processes-memory"),
                &top.by_memory,
                |p| format::size(p.rss),
            ))
            .into(),
        None => muted(widget::text::body(fl!("no-processes"))).into(),
    };
    section(fl!("section-processes"), vec![body])
}

fn process_column(
    title: String,
    list: &[Process],
    value: impl Fn(&Process) -> String,
) -> Element<'_, Message> {
    let mut column = widget::Column::new()
        .spacing(cosmic::theme::spacing().space_xxxs)
        .width(Length::FillPortion(1))
        .push(muted(widget::text::caption_heading(title)));
    for process in list {
        column = column.push(
            widget::Row::new()
                .spacing(cosmic::theme::spacing().space_xxs)
                .push(one_line(widget::text::body(process.name.as_str())).width(Length::Fill))
                .push(widget::text::body(value(process))),
        );
    }
    column.into()
}

/// The two equal buttons under the stats page.
fn footer<'a>() -> Element<'a, Message> {
    widget::Row::new()
        .spacing(cosmic::theme::spacing().space_xs)
        .push(footer_button(
            "utilities-system-monitor-symbolic",
            fl!("process-monitor"),
            Message::OpenProcessMonitor,
        ))
        .push(footer_button(
            "emblem-system-symbolic",
            fl!("settings"),
            Message::ShowPage(Page::Settings),
        ))
        .into()
}

/// A standard button taking an equal share of the row, its icon and label centred. libcosmic's
/// text button can't fill its width.
pub(super) fn footer_button<'a>(
    icon: &'static str,
    label: String,
    message: Message,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let content = widget::Row::new()
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .push(named_icon(icon))
        .push(widget::text::body(label).wrapping(Wrapping::None));
    widget::button::custom(widget::container(content).center_x(Length::Fill))
        .width(Length::Fill)
        .padding([spacing.space_xxs, spacing.space_s])
        .class(cosmic::theme::Button::Standard)
        .on_press(message)
        .into()
}

/// A section: its small header (none for an empty `title`), then `rows`.
pub(super) fn section(title: String, rows: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    widget::Column::new()
        .spacing(spacing.space_xxxs)
        .push(muted(widget::text::caption_heading(title)))
        .extend(rows)
        .into()
}

/// The download/upload or read/write rows of a pair of rates, with their icons and labels.
fn rates<'a>(
    rate: Option<(f64, f64)>,
    labels: [(&'static str, String); 2],
) -> Vec<Element<'a, Message>> {
    let [(first_icon, first), (second_icon, second)] = labels;
    let text = |r: Option<f64>| {
        r.map_or_else(
            || fl!("no-value"),
            |r| fl!("per-second", rate = format::rate(r)),
        )
    };
    vec![
        stat_row(named_icon(first_icon), first, text(rate.map(|r| r.0)), None),
        stat_row(
            named_icon(second_icon),
            second,
            text(rate.map(|r| r.1)),
            None,
        ),
    ]
}

/// An icon, a label, then the value right-aligned in its column and the percentage in its own,
/// in the warning colour when flagged. Without a percentage the value ends at the right edge.
fn stat_row<'a>(
    icon: impl Into<Element<'a, Message>>,
    label: String,
    value: String,
    percent: Option<(String, bool)>,
) -> Element<'a, Message> {
    let mut row = widget::Row::new()
        .spacing(cosmic::theme::spacing().space_xxs)
        .align_y(Alignment::Center)
        .push(icon)
        .push(one_line(widget::text::body(label)).width(Length::Fill));
    match percent {
        Some((percent, warn)) => {
            row = row
                .push(right(widget::text::body(value), VALUE_WIDTH))
                .push(right(
                    warn_if(widget::text::body(percent), warn),
                    PERCENT_WIDTH,
                ));
        }
        None => row = row.push(widget::text::body(value)),
    }
    row.into()
}

/// `text` right-aligned in a column `width` wide.
fn right(text: widget::Text<'_, cosmic::Theme>, width: f32) -> Element<'_, Message> {
    widget::container(text.wrapping(Wrapping::None))
        .width(Length::Fixed(width))
        .align_right(Length::Fixed(width))
        .into()
}

/// System-supplied text on one line, cut with an ellipsis when too long (D8).
fn one_line(text: widget::Text<'_, cosmic::Theme>) -> widget::Text<'_, cosmic::Theme> {
    text.wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
}

fn muted(text: widget::Text<'_, cosmic::Theme>) -> widget::Text<'_, cosmic::Theme> {
    text.class(cosmic::theme::Text::Custom(|theme| {
        let mut color: Color = theme.cosmic().on_bg_color().into();
        color.a *= MUTED_ALPHA;
        text_style(theme, color)
    }))
}

/// `text` in the destructive colour if `warn`, as the panel button shows a warning.
fn warn_if(text: widget::Text<'_, cosmic::Theme>, warn: bool) -> widget::Text<'_, cosmic::Theme> {
    if warn {
        text.class(cosmic::theme::Text::Custom(|theme| {
            text_style(theme, theme.cosmic().destructive_color().into())
        }))
    } else {
        text
    }
}

fn text_style(theme: &cosmic::Theme, color: Color) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(color),
        ..cosmic::iced::widget::text::Catalog::style(theme, &cosmic::theme::Text::Default)
    }
}

fn named_icon<'a>(name: &'static str) -> Element<'a, Message> {
    icon::from_name(name).size(ICON_SIZE).icon().into()
}

fn memory_icon<'a>() -> Element<'a, Message> {
    icon::icon(icons::memory()).size(ICON_SIZE).into()
}

fn temperature_icon<'a>() -> Element<'a, Message> {
    icon::icon(icons::temperature()).size(ICON_SIZE).into()
}

/// A wireless, mobile or wired icon, from the interface's name as systemd names them.
fn interface_icon(name: &str) -> &'static str {
    if name.starts_with("wl") {
        "network-wireless-symbolic"
    } else if name.starts_with("ww") {
        "network-cellular-symbolic"
    } else {
        "network-wired-symbolic"
    }
}

/// "11G / 30G"; nothing used is "0", as a suspended GPU's "0 / 8G" (D4).
fn used_of_total(used: u64, total: u64) -> String {
    fl!(
        "used-of-total",
        used = format::used(used),
        total = format::size(total)
    )
}

/// "up 3d 5h", "up 5h 12m" or "up 12m".
fn uptime(uptime: std::time::Duration) -> String {
    let minutes = uptime.as_secs() / 60;
    let (days, hours, minutes) = (minutes / (24 * 60), minutes / 60 % 24, minutes % 60);
    if days > 0 {
        fl!("uptime-days", days = days, hours = hours)
    } else if hours > 0 {
        fl!("uptime-hours", hours = hours, minutes = minutes)
    } else {
        fl!("uptime-minutes", minutes = minutes)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn uptime_shows_the_two_largest_units() {
        let strip = |s: String| s.replace(['\u{2068}', '\u{2069}'], "");
        assert_eq!(
            strip(uptime(Duration::from_mins((3 * 24 + 5) * 60 + 7))),
            "up 3d 5h"
        );
        assert_eq!(strip(uptime(Duration::from_mins(5 * 60 + 12))), "up 5h 12m");
        assert_eq!(strip(uptime(Duration::from_secs(12 * 60 + 59))), "up 12m");
    }

    #[test]
    fn a_suspended_gpu_s_vram_reads_0_of_the_rounded_total() {
        let strip = |s: String| s.replace(['\u{2068}', '\u{2069}'], "");
        assert_eq!(strip(used_of_total(0, 8188 << 20)), "0 / 8G");
        assert_eq!(strip(used_of_total(512 << 20, 8188 << 20)), "512M / 8G");
    }

    #[test]
    fn interfaces_get_icons_by_kind() {
        assert_eq!(interface_icon("wlp4s0"), "network-wireless-symbolic");
        assert_eq!(interface_icon("wwan0"), "network-cellular-symbolic");
        assert_eq!(interface_icon("eno1"), "network-wired-symbolic");
    }
}
