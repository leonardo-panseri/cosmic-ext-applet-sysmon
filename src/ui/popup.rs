// SPDX-License-Identifier: MPL-2.0

//! The popup (plan §2.2, §2.3): the stats page in a scrollable area over a footer with the
//! **Process monitor** and **Settings** buttons, or the settings page. In two columns, CPU, memory
//! and GPU sit beside network, disk and processes, a divider between them, over the same footer.
//!
//! Everything here lays out the latest [`Stats`] and reads nothing else, so a redraw costs only
//! the layout. All system-supplied text (models, interface names, mount points, process names) is
//! plain text, clipped to one line (D8).
//!
//! A section with one block per device (each GPU, each disk, each interface) sets the blocks
//! apart with space and an inset divider, lighter than the dividers between sections, and heads
//! each with the device's name in bold.
//!
//! Hovering a CPU, memory or GPU value shows its lowest and highest since the applet started, in
//! iced's own tooltip: an overlay inside the popup's surface, so it needs no surface of its own,
//! and it redraws only when it opens or closes.

use std::time::Duration;

use cosmic::iced::advanced::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget::tooltip::Position;
use cosmic::widget::{self, icon};

use crate::app::Message;
use crate::fl;
use crate::panel::{LimitKind, PanelSettings, format};
use crate::sampler::stats::{DiskStats, GpuSample, MinMax, Processes, Stats, share};
use crate::ui::columns::Columns;
use crate::ui::settings::{self, SettingsModels};
use crate::ui::{builder, graph, icons};
use sysmon_proto::{MAX_TOP, Mount, Process, TopProcesses};

/// The width of the value column of a stat row: "212G / 928G".
const VALUE_WIDTH: f32 = 104.0;
/// The width of the percentage column: "100%".
const PERCENT_WIDTH: f32 = 44.0;
const ICON_SIZE: u16 = 16;
/// A core at or above this load, %, shows in the warning colour, as in the reference.
const BUSY_CORE: f32 = 85.0;
/// The opacity of secondary text, relative to the text colour.
const MUTED_ALPHA: f32 = 0.65;
/// How long the cursor rests on a value before its min/max tooltip opens, so sweeping across
/// the popup doesn't flash them.
const TOOLTIP_DELAY: Duration = Duration::from_millis(400);

/// Which page the popup shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Stats,
    /// The panel button's settings (§2.3).
    Settings,
    /// The panel button's builder (`docs/panel-builder.md`), from the settings page.
    Builder,
}

/// The popup's content, [`Columns::width`] wide: `page`, for `stats` (`None` before the first
/// sample), with values at or above `settings`' limits in the warning colour. The stats page
/// scrolls beyond `max_height`; the settings page shows `settings` with the segmented controls
/// of `models`.
pub fn view<'a>(
    applet: &cosmic::applet::Context,
    stats: Option<&'a Stats>,
    settings: &PanelSettings,
    models: &'a SettingsModels,
    page: Page,
    columns: Columns,
    max_height: f32,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let content: Element<'a, Message> = match page {
        Page::Stats => {
            let body = match stats {
                Some(stats) => stats_page(stats, settings, columns),
                None => widget::text::body(fl!("placeholder")).into(),
            };
            let body = widget::container(body).padding([0, spacing.space_s]);
            widget::Column::new()
                .push(widget::container(widget::scrollable(body)).max_height(max_height))
                .push(widget::container(footer()).padding([0, spacing.space_m]))
                .spacing(spacing.space_s)
                .into()
        }
        // Padded inside, around its scrollable area.
        Page::Settings => settings::view(models, settings, columns, max_height),
        Page::Builder => builder::view(applet, stats, settings, models, columns, max_height),
    };
    widget::container(content)
        .padding([spacing.space_s, 0])
        .width(Length::Fixed(columns.width()))
        .into()
}

/// The sections of §2.2, in the reference's order, between dividers: in one column, or the first
/// three beside the rest.
fn stats_page<'a>(
    stats: &'a Stats,
    settings: &PanelSettings,
    columns: Columns,
) -> Element<'a, Message> {
    let mut first = first_sections(stats, settings);
    let second = second_sections(stats, settings);
    match columns {
        Columns::One => {
            first.extend(second);
            with_dividers(first)
        }
        Columns::Two => side_by_side(with_dividers(first), with_dividers(second)),
    }
}

/// CPU, memory and the GPUs, if any.
fn first_sections<'a>(stats: &'a Stats, settings: &PanelSettings) -> Vec<Element<'a, Message>> {
    [
        Some(cpu(stats, settings)),
        Some(memory(stats, settings)),
        (!stats.gpus.is_empty()).then(|| gpus(stats, settings)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Network, disk and processes.
fn second_sections<'a>(stats: &'a Stats, settings: &PanelSettings) -> Vec<Element<'a, Message>> {
    vec![network(stats), disk(stats, settings), processes(stats)]
}

/// Two columns of equal width, tops aligned, with a divider between them in the space of the
/// popup's padding either side.
pub(super) fn side_by_side<'a>(
    first: Element<'a, Message>,
    second: Element<'a, Message>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    widget::Row::new()
        .spacing(spacing.space_s)
        .push(widget::container(first).width(Length::FillPortion(1)))
        .push(widget::divider::vertical::default())
        .push(widget::container(second).width(Length::FillPortion(1)))
        .into()
}

/// `sections` between dividers. As in COSMIC's applets, the dividers reach `space_xxs` further
/// out than the sections: the sections sit `space_m` from the popup's edges, the dividers
/// `space_s`.
fn with_dividers(sections: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let mut page = widget::Column::new().spacing(spacing.space_xs);
    for (i, section) in sections.into_iter().enumerate() {
        if i > 0 {
            page = page.push(widget::divider::horizontal::default());
        }
        page = page.push(widget::container(section).padding([0, spacing.space_xxs]));
    }
    page.into()
}

/// Big total load and uptime on the left; thread count, load average and package temperature on
/// the right; then the graph and the per-core bars.
fn cpu<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let left = widget::Column::new()
        .push(since_start(
            widget::text::title2(stats.cpu.map_or_else(|| fl!("no-value"), format::percent)),
            stats.extremes.cpu,
            format::percent,
            Position::Bottom,
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
        right = right.push(since_start(
            warn_if(
                widget::text::body(format::celsius(temp)),
                settings.warns(LimitKind::CpuTemp, temp),
            ),
            stats.extremes.cpu_temp,
            format::celsius,
            Position::Bottom,
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

fn memory<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let mut rows = Vec::new();
    if let Some(mem) = stats.mem {
        let percent = stats.memory_percent();
        rows.push(since_start(
            stat_row(
                memory_icon(),
                fl!("in-use"),
                used_of_total(mem.mem_used(), mem.mem_total),
                percent.map(|p| (format::percent(p), settings.warns(LimitKind::Memory, p))),
            ),
            stats.extremes.memory,
            bytes,
            Position::Top,
        ));
        let swap = share(mem.swap_used(), mem.swap_total).unwrap_or(0.0);
        rows.push(since_start(
            stat_row(
                named_icon("media-flash-symbolic"),
                fl!("swap"),
                used_of_total(mem.swap_used(), mem.swap_total),
                Some((format::percent(swap), settings.warns(LimitKind::Swap, swap))),
            ),
            stats.extremes.swap,
            bytes,
            Position::Top,
        ));
        // In use above the line, swap below.
        rows.push(graph::mirrored_percent(
            &stats.history.memory,
            &stats.history.swap,
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
    let gpus = stats.gpus_nvidia_first().map(|gpu| {
        let mut rows = vec![gpu_header(gpu, settings)];
        if gpu.busy.is_some()
            && let Some(history) = history(&gpu.card)
        {
            rows.push(graph::percent(history));
        }
        if let (Some(used), Some(total)) = (gpu.vram_used, gpu.vram_total) {
            let percent = share(used, total).unwrap_or(0.0);
            rows.push(since_start(
                stat_row(
                    memory_icon(),
                    fl!("vram"),
                    used_of_total(used, total),
                    Some((
                        format::percent(percent),
                        settings.warns(LimitKind::Vram, percent),
                    )),
                ),
                gpu.extremes.vram,
                bytes,
                Position::Top,
            ));
        }
        block(rows)
    });
    section(fl!("section-gpu"), vec![blocks(gpus)])
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
            parts.push(since_start(
                widget::text::body(format::percent(f32::from(busy))),
                gpu.extremes.busy,
                format::percent,
                Position::Top,
            ));
        }
        if let Some(temp) = gpu.celsius {
            parts.push(since_start(
                warn_if(
                    widget::text::body(format::celsius(temp)),
                    settings.warns(LimitKind::GpuTemp, temp),
                ),
                gpu.extremes.celsius,
                format::celsius,
                Position::Top,
            ));
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
    block_header(named_icon("video-display-symbolic"), name, values.into())
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
    let mut rows = Vec::new();
    if in_use.len() > 1 {
        let interfaces = in_use.into_iter().map(|history| {
            let rate = stats
                .net
                .iter()
                .find(|d| d.name == history.name)
                .and_then(|d| d.rate);
            let mut rows = vec![block_header(
                named_icon(interface_icon(&history.name)),
                history.name.clone(),
                widget::Space::new().into(),
            )];
            rows.extend(rates(
                rate.map(|r| (r.first, r.second)),
                [
                    ("network-receive-symbolic", fl!("download-label")),
                    ("network-transmit-symbolic", fl!("upload-label")),
                ],
            ));
            rows.push(graph::mirrored(&history.down, &history.up));
            block(rows)
        });
        rows.push(blocks(interfaces));
    } else {
        rows.extend(rates(
            stats.net_total.map(|r| (r.first, r.second)),
            [
                ("network-receive-symbolic", fl!("download-label")),
                ("network-transmit-symbolic", fl!("upload-label")),
            ],
        ));
        rows.push(graph::mirrored(
            &stats.history.net_down,
            &stats.history.net_up,
        ));
    }
    section(fl!("section-network"), rows)
}

/// One block per physical disk: its model and temperature, its own read and write rates, and
/// its filesystems with their space; then the filesystems on no counted disk.
fn disk<'a>(stats: &'a Stats, settings: &PanelSettings) -> Element<'a, Message> {
    let mut disks: Vec<Element<'a, Message>> = stats
        .disks
        .iter()
        .map(|disk| disk_block(disk, settings))
        .collect();
    if !stats.other_mounts.is_empty() {
        let mut rows = vec![block_header(
            named_icon("drive-multidisk-symbolic"),
            fl!("disk-other"),
            widget::Space::new().into(),
        )];
        rows.extend(stats.other_mounts.iter().map(|m| mount_rows(m, settings)));
        disks.push(block(rows));
    }
    let rows = if disks.is_empty() {
        // Nothing found, as in a container: the total, if any.
        vec![disk_rates(stats.disk_total.map(|r| (r.first, r.second)))]
    } else {
        vec![blocks(disks)]
    };
    section(fl!("section-disk"), rows)
}

fn disk_block<'a>(disk: &'a DiskStats, settings: &PanelSettings) -> Element<'a, Message> {
    let temp: Element<'a, Message> = match &disk.temp {
        Some(t) => since_start(
            widget::Row::new()
                .spacing(cosmic::theme::spacing().space_xxxs)
                .align_y(Alignment::Center)
                .push(temperature_icon())
                .push(warn_if(
                    widget::text::body(format::celsius(t.celsius)),
                    settings.nvme_hot(t),
                )),
            t.extremes,
            format::celsius,
            Position::Top,
        ),
        None => widget::Space::new().into(),
    };
    let mut rows = vec![block_header(
        named_icon(disk_icon(&disk.name)),
        disk.model.clone(),
        temp,
    )];
    if !disk.name.is_empty() {
        rows.push(disk_rates(disk.rate.map(|r| (r.first, r.second))));
    }
    rows.extend(disk.mounts.iter().map(|m| mount_rows(m, settings)));
    block(rows)
}

/// Read and write on one row, side by side.
fn disk_rates<'a>(rate: Option<(f64, f64)>) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let half = |icon: &'static str, label: String, rate: Option<f64>| {
        widget::Row::new()
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center)
            .width(Length::FillPortion(1))
            .push(named_icon(icon))
            .push(one_line(widget::text::body(label)).width(Length::Fill))
            .push(widget::text::body(rate_text(rate)).wrapping(Wrapping::None))
    };
    widget::Row::new()
        .spacing(spacing.space_m)
        .push(half(
            "document-open-symbolic",
            fl!("read"),
            rate.map(|r| r.0),
        ))
        .push(half(
            "document-save-symbolic",
            fl!("write"),
            rate.map(|r| r.1),
        ))
        .into()
}

/// A filesystem's row, then its usage bar under the label.
fn mount_rows<'a>(mount: &'a Mount, settings: &PanelSettings) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let percent = mount.used_percent();
    let full = percent.is_some_and(|p| settings.warns(LimitKind::Disk, p));
    let row = stat_row(
        named_icon("folder-symbolic"),
        mount.mount_point.clone(),
        used_of_total(mount.used, mount.total),
        percent.map(|p| (format::percent(p), full)),
    );
    let bar = widget::Row::new()
        .push(widget::Space::new().width(Length::Fixed(f32::from(ICON_SIZE + spacing.space_xxs))))
        .push(graph::usage_bar(percent.unwrap_or(0.0), full));
    widget::Column::new()
        .spacing(spacing.space_xxxs)
        .push(row)
        .push(bar)
        .into()
}

/// The top processes by CPU and by memory, side by side. Until the popup's first scan answers
/// (the probe takes a 250 ms CPU baseline), a ghost of the same height holds their place, so
/// the page doesn't shift when they arrive.
fn processes(stats: &Stats) -> Element<'_, Message> {
    let body: Element<'_, Message> = match &stats.processes {
        Processes::Top(top) => process_columns(Some(top)),
        Processes::Pending => process_columns(None),
        Processes::Unavailable => muted(widget::text::body(fl!("no-processes"))).into(),
    };
    section(fl!("section-processes"), vec![body])
}

/// The CPU and memory columns of `top`, or their ghosts without it.
fn process_columns(top: Option<&TopProcesses>) -> Element<'_, Message> {
    widget::Row::new()
        .spacing(cosmic::theme::spacing().space_m)
        .push(process_column(
            fl!("processes-cpu"),
            top.map(|t| t.by_cpu.as_slice()),
            |p| format::fine_percent(p.cpu),
        ))
        .push(process_column(
            fl!("processes-memory"),
            top.map(|t| t.by_memory.as_slice()),
            |p| format::size(p.rss),
        ))
        .into()
}

/// A column headed `title`: the processes of `list` with their value, or [`MAX_TOP`] ghost rows
/// without one.
fn process_column(
    title: String,
    list: Option<&[Process]>,
    value: impl Fn(&Process) -> String,
) -> Element<'_, Message> {
    let mut column = widget::Column::new()
        .spacing(cosmic::theme::spacing().space_xxxs)
        .width(Length::FillPortion(1))
        .push(muted(widget::text::caption_heading(title)));
    let Some(list) = list else {
        return column
            .extend(GHOST_NAME_PORTIONS.iter().map(|&p| ghost_row(p)))
            .into();
    };
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

/// How much of a ghost row its name bar takes, in tenths, one per row: uneven, like names.
const GHOST_NAME_PORTIONS: [u16; MAX_TOP as usize] = [7, 5, 6, 4, 5];
/// A ghost bar's height and its value bar's width.
const GHOST_BAR_HEIGHT: f32 = 8.0;
const GHOST_VALUE_WIDTH: f32 = 32.0;
/// A ghost bar's opacity, relative to the text colour.
const GHOST_ALPHA: f32 = 0.12;

/// A process row's ghost: a name bar `name` tenths wide and a value bar, as high as a row of
/// body text (a no-break space keeps that height; a zero-width text has none).
fn ghost_row<'a>(name: u16) -> Element<'a, Message> {
    let bars = widget::Row::new()
        .spacing(cosmic::theme::spacing().space_xxs)
        .push(ghost_bar(Length::FillPortion(name)))
        .push(widget::Space::new().width(Length::FillPortion(10 - name)))
        .push(ghost_bar(Length::Fixed(GHOST_VALUE_WIDTH)));
    // The space last, where its few pixels of width don't show.
    widget::Row::new()
        .align_y(Alignment::Center)
        .push(bars.width(Length::Fill))
        .push(widget::text::body("\u{a0}"))
        .into()
}

fn ghost_bar<'a>(width: Length) -> Element<'a, Message> {
    widget::container(widget::Space::new())
        .width(width)
        .height(Length::Fixed(GHOST_BAR_HEIGHT))
        .style(|theme: &cosmic::Theme| {
            let mut color: Color = theme.cosmic().background(theme.transparent).on.into();
            color.a *= GHOST_ALPHA;
            widget::container::Style {
                background: Some(color.into()),
                border: cosmic::iced::Border {
                    radius: (GHOST_BAR_HEIGHT / 2.0).into(),
                    ..cosmic::iced::Border::default()
                },
                ..widget::container::Style::default()
            }
        })
        .into()
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
    vec![
        stat_row(
            named_icon(first_icon),
            first,
            rate_text(rate.map(|r| r.0)),
            None,
        ),
        stat_row(
            named_icon(second_icon),
            second,
            rate_text(rate.map(|r| r.1)),
            None,
        ),
    ]
}

/// "1.2M/s", or a dash without a rate.
fn rate_text(rate: Option<f64>) -> String {
    rate.map_or_else(
        || fl!("no-value"),
        |r| fl!("per-second", rate = format::rate(r)),
    )
}

/// The blocks of a section with one per device, set apart by space and an inset divider.
fn blocks<'a>(blocks: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
    let mut column = widget::Column::new().spacing(cosmic::theme::spacing().space_xs);
    for (i, block) in blocks.into_iter().enumerate() {
        if i > 0 {
            column = column.push(widget::divider::horizontal::light());
        }
        column = column.push(block);
    }
    column.into()
}

/// One device's rows.
fn block(rows: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    widget::Column::new()
        .spacing(cosmic::theme::spacing().space_xxxs)
        .extend(rows)
        .into()
}

/// A block's first row: an icon, the device's name in bold, and `values` at the right edge.
fn block_header<'a>(
    icon: Element<'a, Message>,
    name: String,
    values: Element<'a, Message>,
) -> Element<'a, Message> {
    widget::Row::new()
        .spacing(cosmic::theme::spacing().space_xxs)
        .align_y(Alignment::Center)
        .push(icon)
        .push(one_line(widget::text::heading(name)).width(Length::Fill))
        .push(values)
        .into()
}

/// `content` with a tooltip, at `position`, of the lowest and highest values since start, if
/// there are any yet.
fn since_start<'a>(
    content: impl Into<Element<'a, Message>>,
    range: Option<MinMax>,
    format: fn(f32) -> String,
    position: Position,
) -> Element<'a, Message> {
    let Some(range) = range else {
        return content.into();
    };
    widget::tooltip(
        content,
        widget::text::caption(since_start_text(range, format)),
        position,
    )
    .delay(TOOLTIP_DELAY)
    .into()
}

/// A size held as an [`MinMax`] value, as the rows write it: "5.2G".
fn bytes(value: f32) -> String {
    // Sizes recorded from `u64`s: never negative, and far below `u64::MAX`.
    #[expect(clippy::cast_possible_truncation, reason = "see above")]
    #[expect(clippy::cast_sign_loss, reason = "see above")]
    format::used(value.round() as u64)
}

/// "Since start: min 3% · max 97%".
fn since_start_text(range: MinMax, format: fn(f32) -> String) -> String {
    fl!(
        "since-start",
        min = format(range.min),
        max = format(range.max)
    )
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
pub(super) fn one_line(text: widget::Text<'_, cosmic::Theme>) -> widget::Text<'_, cosmic::Theme> {
    text.wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
}

pub(super) fn muted(text: widget::Text<'_, cosmic::Theme>) -> widget::Text<'_, cosmic::Theme> {
    text.class(cosmic::theme::Text::Custom(|theme| {
        let mut color: Color = theme.cosmic().background(theme.transparent).on.into();
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

/// A solid-state icon for `NVMe` and MMC disks, else a hard disk.
fn disk_icon(name: &str) -> &'static str {
    if name.starts_with("nvme") || name.starts_with("mmcblk") || name.is_empty() {
        "drive-harddisk-solidstate-symbolic"
    } else {
        "drive-harddisk-system-symbolic"
    }
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
    fn the_min_max_tooltip_shows_both_extremes_formatted() {
        let strip = |s: String| s.replace(['\u{2068}', '\u{2069}'], "");
        let range = MinMax {
            min: 3.2,
            max: 96.6,
        };
        assert_eq!(
            strip(since_start_text(range, format::percent)),
            "Since start: min 3% · max 97%"
        );
        let range = MinMax {
            min: 41.0,
            max: 88.4,
        };
        assert_eq!(
            strip(since_start_text(range, format::celsius)),
            "Since start: min 41°C · max 88°C"
        );
        #[expect(clippy::cast_precision_loss, reason = "test sizes")]
        let range = MinMax {
            min: (5_600u64 << 20) as f32,
            max: (12u64 << 30) as f32,
        };
        assert_eq!(
            strip(since_start_text(range, bytes)),
            "Since start: min 5.5G · max 12G"
        );
    }

    #[test]
    fn interfaces_get_icons_by_kind() {
        assert_eq!(interface_icon("wlp4s0"), "network-wireless-symbolic");
        assert_eq!(interface_icon("wwan0"), "network-cellular-symbolic");
        assert_eq!(interface_icon("eno1"), "network-wired-symbolic");
    }
}
