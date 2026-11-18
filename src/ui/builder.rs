// SPDX-License-Identifier: MPL-2.0

//! The panel button's builder (`docs/panel-builder.md`), a sub-page of the settings page: a live
//! preview of the panel button, its layout, and its two rows as chips. Selecting a chip offers
//! to move it, move it to the other row or remove it, and to choose its GPU, mount, disk, drive
//! or interface; **+** opens a picker of every item there is, grouped. Every change saves at once,
//! as the settings page's do.

use cosmic::applet::Context;
use cosmic::iced::advanced::text::Wrapping;
use cosmic::iced::{Alignment, Color, Length};
use cosmic::prelude::*;
use cosmic::widget::{self, icon, segmented_control};

use super::columns::Columns;
use super::popup::{self, Page};
use super::settings::{SettingChange, SettingsModels};
use crate::app::Message;
use crate::fl;
use crate::panel::{
    DiskId, DiskRef, GpuRef, Group, Item, NetRef, PanelSettings, PanelSummary, Row, Rows, Slot,
    choices, find_disk,
};
use crate::sampler::stats::{GpuSample, Stats};
use crate::ui::icons::{self, Glyph};
use crate::ui::panel_button::panel_button;

/// A change made on the builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuilderChange {
    /// A chip clicked: selects it, or unselects it if it was selected.
    Select(Slot),
    /// ✕ on a chip, or **Remove**.
    Remove(Slot),
    /// **Move left**, or **Move right** if `true`.
    Shift(Slot, bool),
    /// **Move to the other row**.
    ToOtherRow(Slot),
    /// Another GPU, mount, disk, drive or interface for the item at the slot.
    Retarget(Slot, Item),
    /// **+** at the end of a row.
    OpenPicker(Row),
    ClosePicker,
    /// An entry of the picker, for the row it was opened for.
    Add(Row, Item),
}

/// What the builder shows beyond the settings: the selected chip, or the picker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Builder {
    selected: Option<Slot>,
    /// The row the picker adds to, while it is open.
    adding: Option<Row>,
}

impl Builder {
    /// The selected chip, if any.
    #[must_use]
    pub fn selected(&self) -> Option<Slot> {
        self.selected
    }

    /// The row the open picker adds to.
    #[must_use]
    pub fn adding(&self) -> Option<Row> {
        self.adding
    }

    /// Makes `change` to `rows`, keeping the selection on the item it was on.
    pub fn change(&mut self, change: BuilderChange, rows: &mut Rows) {
        match change {
            BuilderChange::Select(slot) => {
                self.adding = None;
                self.selected =
                    (self.selected != Some(slot) && rows.get(slot).is_some()).then_some(slot);
            }
            BuilderChange::Remove(slot) => {
                if rows.remove(slot).is_some() {
                    self.selected = match self.selected {
                        Some(s) if s == slot => None,
                        Some(s) if s.row == slot.row && s.index > slot.index => Some(Slot {
                            index: s.index - 1,
                            ..s
                        }),
                        other => other,
                    };
                }
            }
            BuilderChange::Shift(slot, right) => {
                if let Some(to) = rows.shift(slot, right) {
                    self.selected = Some(to);
                }
            }
            BuilderChange::ToOtherRow(slot) => {
                if let Some(to) = rows.move_to_other_row(slot) {
                    self.selected = Some(to);
                }
            }
            BuilderChange::Retarget(slot, item) => {
                if rows.get(slot).is_some_and(|old| old.same_kind(&item)) {
                    rows.set(slot, item);
                }
            }
            BuilderChange::OpenPicker(row) => {
                if rows.has_room(row) {
                    self.selected = None;
                    self.adding = Some(row);
                }
            }
            BuilderChange::ClosePicker => self.adding = None,
            BuilderChange::Add(row, item) => {
                rows.push(row, item);
                self.adding = None;
            }
        }
    }

    /// Back to nothing selected and no picker, as when the page opens.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The builder page: the preview, the layout and the rows, then what the selection or the
/// picker offers; in two columns, those beside the rest. Scrolls beyond `max_height`.
pub fn view<'a>(
    applet: &Context,
    stats: Option<&'a Stats>,
    settings: &PanelSettings,
    models: &'a SettingsModels,
    columns: Columns,
    max_height: f32,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let fallback = Stats::default();
    let stats_or_default = stats.unwrap_or(&fallback);
    let builder = models.builder();
    let summary = stats.map(|stats| PanelSummary::new(stats, settings));
    let layout_choice = popup::section(
        fl!("setting-layout"),
        vec![
            segmented_control::horizontal(models.layout())
                .on_activate(|item| Message::Setting(SettingChange::Layout(item)))
                .button_padding([spacing.space_xxs, 0, spacing.space_xxs, 0])
                .width(Length::Fill)
                .into(),
        ],
    );
    let main = vec![
        preview(panel_button(applet, settings, summary.as_ref())),
        layout_choice,
        row_chips(
            fl!("setting-top-row"),
            Row::Top,
            settings,
            stats_or_default,
            builder,
        ),
        row_chips(
            fl!("setting-bottom-row"),
            Row::Bottom,
            settings,
            stats_or_default,
            builder,
        ),
    ];
    let detail = match (builder.adding(), builder.selected()) {
        (Some(row), _) => picker(row, stats_or_default),
        (None, Some(slot)) => match settings.rows.get(slot) {
            Some(item) => selection(slot, item, &settings.rows, stats_or_default),
            None => hint(),
        },
        (None, None) => hint(),
    };
    let column = |parts: Vec<Element<'a, Message>>| {
        widget::Column::new()
            .spacing(spacing.space_s)
            .extend(parts)
            .width(Length::Fill)
    };
    let page: Element<'a, Message> = match columns {
        Columns::One => column(main)
            .push(widget::divider::horizontal::default())
            .push(detail)
            .into(),
        Columns::Two => popup::side_by_side(column(main).into(), column(vec![detail]).into()),
    };
    // `space_m` from the edges, as the stats page's sections and COSMIC's applets.
    let page = widget::container(page).padding([0, spacing.space_m]);
    let padded =
        |content: Element<'a, Message>| widget::container(content).padding([0, spacing.space_m]);
    widget::Column::new()
        .spacing(spacing.space_s)
        .push(padded(widget::text::title4(fl!("panel-button")).into()))
        .push(
            widget::container(widget::scrollable(page))
                .max_height((max_height - super::settings::TITLE_HEIGHT).max(0.0)),
        )
        .push(padded(popup::footer_button(
            "go-previous-symbolic",
            fl!("back"),
            Message::ShowPage(Page::Settings),
        )))
        .into()
}

/// The panel button as the panel shows it, on a strip of the panel's colour. Pressing it does
/// nothing.
fn preview(button: widget::Button<'_, Message>) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    widget::container(button.on_press(Message::ShowPage(Page::Builder)))
        .center_x(Length::Fill)
        .padding([spacing.space_xxs, spacing.space_s])
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            widget::container::Style {
                text_color: Some(cosmic.background(theme.transparent).on.into()),
                icon_color: Some(cosmic.background(theme.transparent).on.into()),
                background: Some(Color::from(cosmic.background(theme.transparent).base).into()),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..widget::container::Style::default()
            }
        }))
        .into()
}

/// A row's caption over its chips and **+**, wrapping.
fn row_chips<'a>(
    title: String,
    row: Row,
    settings: &PanelSettings,
    stats: &Stats,
    builder: &Builder,
) -> Element<'a, Message> {
    let mut chips: Vec<Element<'a, Message>> = settings
        .rows
        .row(row)
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let slot = Slot { row, index };
            chip(slot, item, stats, builder.selected() == Some(slot))
        })
        .collect();
    let add = widget::button::icon(icon::from_name("list-add-symbolic"))
        .on_press_maybe(settings.rows.has_room(row).then_some(Message::Setting(
            SettingChange::Builder(BuilderChange::OpenPicker(row)),
        )))
        .selected(builder.adding() == Some(row));
    chips.push(
        widget::tooltip(
            add,
            widget::text::body(fl!("builder-add")),
            widget::tooltip::Position::Top,
        )
        .into(),
    );
    let spacing = cosmic::theme::spacing();
    popup::section(
        title,
        vec![
            widget::flex_row(chips)
                .row_spacing(spacing.space_xxs)
                .column_spacing(spacing.space_xxs)
                .align_items(Alignment::Center)
                .into(),
        ],
    )
}

/// One item as a chip: its glyph, short name and live value, which selects it when clicked, and
/// ✕, which removes it. A selected chip is outlined in the accent colour.
fn chip<'a>(slot: Slot, item: &Item, stats: &Stats, selected: bool) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let value = item.value(stats).unwrap_or_else(|| fl!("no-value"));
    let label = widget::Row::new()
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .push(small_glyph(item))
        .push(widget::text::body(short_name(item, stats)).wrapping(Wrapping::None))
        .push(popup::muted(
            widget::text::body(value).wrapping(Wrapping::None),
        ));
    let select = widget::button::custom(label)
        .padding([spacing.space_xxxs, spacing.space_xxs])
        .class(chip_button())
        .on_press(Message::Setting(SettingChange::Builder(
            BuilderChange::Select(slot),
        )));
    let remove = widget::button::icon(icon::from_name("window-close-symbolic"))
        .extra_small()
        .on_press(Message::Setting(SettingChange::Builder(
            BuilderChange::Remove(slot),
        )));
    widget::container(
        widget::Row::new()
            .align_y(Alignment::Center)
            .push(select)
            .push(remove),
    )
    .padding([0, spacing.space_xxxs, 0, 0])
    .class(cosmic::theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        widget::container::Style {
            text_color: Some(component.on.into()),
            icon_color: Some(component.on.into()),
            background: Some(Color::from(component.base).into()),
            border: cosmic::iced::Border {
                radius: cosmic.corner_radii.radius_xl.into(),
                width: if selected { 2.0 } else { 1.0 },
                color: if selected {
                    cosmic.accent.base.into()
                } else {
                    component.divider.into()
                },
            },
            ..widget::container::Style::default()
        }
    }))
    .into()
}

/// A chip's own button: the text colour, and the hover colour only while hovered or pressed,
/// rounded as the chip is.
fn chip_button() -> cosmic::theme::Button {
    fn style(theme: &cosmic::Theme, hovered: bool) -> widget::button::Style {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        widget::button::Style {
            background: hovered.then(|| Color::from(component.hover).into()),
            border_radius: cosmic.corner_radii.radius_xl.into(),
            text_color: Some(component.on.into()),
            icon_color: Some(component.on.into()),
            ..widget::button::Style::new()
        }
    }
    cosmic::theme::Button::Custom {
        active: Box::new(|_, theme| style(theme, false)),
        disabled: Box::new(|theme| style(theme, false)),
        hovered: Box::new(|_, theme| style(theme, true)),
        pressed: Box::new(|_, theme| style(theme, true)),
    }
}

/// What the detail area shows with nothing selected.
fn hint<'a>() -> Element<'a, Message> {
    popup::muted(widget::text::body(fl!("builder-hint"))).into()
}

/// The selected item: its name, the moves and **Remove**, then its targets to choose from.
fn selection<'a>(slot: Slot, item: &Item, rows: &Rows, stats: &Stats) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let change = |change: BuilderChange| Message::Setting(SettingChange::Builder(change));
    let len = rows.row(slot.row).len();
    let action = |icon_name: &'static str, label: String, message: Option<Message>| {
        widget::button::standard(label)
            .leading_icon(icon::from_name(icon_name))
            .on_press_maybe(message)
            .into()
    };
    let (other_icon, other_label) = match slot.row {
        Row::Top => ("go-down-symbolic", fl!("builder-to-bottom-row")),
        Row::Bottom => ("go-up-symbolic", fl!("builder-to-top-row")),
    };
    let actions: Vec<Element<'a, Message>> = vec![
        action(
            "go-previous-symbolic",
            fl!("builder-move-left"),
            (slot.index > 0).then(|| change(BuilderChange::Shift(slot, false))),
        ),
        action(
            "go-next-symbolic",
            fl!("builder-move-right"),
            (slot.index + 1 < len).then(|| change(BuilderChange::Shift(slot, true))),
        ),
        action(
            other_icon,
            other_label,
            rows.has_room(slot.row.other())
                .then(|| change(BuilderChange::ToOtherRow(slot))),
        ),
        action(
            "edit-delete-symbolic",
            fl!("builder-remove"),
            Some(change(BuilderChange::Remove(slot))),
        ),
    ];
    let head = widget::Row::new()
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center)
        .push(small_glyph(item))
        .push(popup::one_line(widget::text::heading(full_name(item, stats))).width(Length::Fill));
    let mut column = widget::Column::new()
        .spacing(spacing.space_s)
        .push(head)
        .push(
            widget::flex_row(actions)
                .row_spacing(spacing.space_xxs)
                .column_spacing(spacing.space_xxs),
        );
    if item.has_target() {
        let targets = item.targets(stats);
        let current = targets.iter().position(|t| t == item);
        let radios = targets
            .into_iter()
            .enumerate()
            .map(|(i, target)| {
                let label = target_name(&target, stats);
                let value = target.value(stats).unwrap_or_else(|| fl!("no-value"));
                let label = widget::Row::new()
                    .spacing(spacing.space_xxs)
                    .push(popup::one_line(widget::text::body(label)).width(Length::Fill))
                    .push(popup::muted(widget::text::body(value)));
                widget::radio(label, i, current, move |_| {
                    change(BuilderChange::Retarget(slot, target.clone()))
                })
                .width(Length::Fill)
                .into()
            })
            .collect();
        column = column.push(popup::section(target_title(item), radios));
    }
    column.into()
}

/// The picker for `row`: every item there is, one entry per target, under the group captions,
/// each with its live value; and a way to close it.
fn picker<'a>(row: Row, stats: &Stats) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let change = |change: BuilderChange| Message::Setting(SettingChange::Builder(change));
    let title = match row {
        Row::Top => fl!("builder-add-top"),
        Row::Bottom => fl!("builder-add-bottom"),
    };
    let head = widget::Row::new()
        .align_y(Alignment::Center)
        .push(widget::text::heading(title).width(Length::Fill))
        .push(
            widget::button::icon(icon::from_name("window-close-symbolic"))
                .on_press(change(BuilderChange::ClosePicker)),
        );
    let items = choices(stats);
    let mut column = widget::Column::new().spacing(spacing.space_s).push(head);
    for group in Group::ALL {
        let entries: Vec<Element<'a, Message>> = items
            .iter()
            .filter(|item| item.group() == group)
            .map(|item| {
                let value = item.value(stats).unwrap_or_else(|| fl!("no-value"));
                let content = widget::Row::new()
                    .spacing(spacing.space_xs)
                    .align_y(Alignment::Center)
                    .push(small_glyph(item))
                    .push(
                        popup::one_line(widget::text::body(full_name(item, stats)))
                            .width(Length::Fill),
                    )
                    .push(popup::muted(widget::text::body(value)));
                widget::button::custom(content)
                    .width(Length::Fill)
                    .padding([spacing.space_xxs, spacing.space_xs])
                    .class(cosmic::theme::Button::MenuItem)
                    .on_press(change(BuilderChange::Add(row, item.clone())))
                    .into()
            })
            .collect();
        if !entries.is_empty() {
            column = column.push(popup::section(group_title(group), entries));
        }
    }
    column.into()
}

/// An item's glyph at the popup's icon size.
fn small_glyph<'a>(item: &Item) -> Element<'a, Message> {
    let glyph: Element<'a, Message> = match icons::item(item) {
        Glyph::Icon(handle) => icon::icon(handle).size(16).into(),
        Glyph::Text(text) => widget::text::body(text).into(),
    };
    widget::container(glyph)
        .center_x(Length::Fixed(16.0))
        .into()
}

fn group_title(group: Group) -> String {
    match group {
        Group::Cpu => fl!("section-cpu"),
        Group::Memory => fl!("section-memory"),
        Group::Gpu => fl!("section-gpu"),
        Group::Disk => fl!("section-disk"),
        Group::Network => fl!("section-network"),
    }
}

/// The caption over an item's targets.
fn target_title(item: &Item) -> String {
    match item {
        Item::DiskUsage(_) => fl!("builder-target-mount"),
        Item::DiskRead(_) | Item::DiskWrite(_) => fl!("builder-target-disk"),
        Item::NvmeTemp(_) => fl!("builder-target-drive"),
        Item::Download(_) | Item::Upload(_) => fl!("builder-target-interface"),
        _ => fl!("section-gpu"),
    }
}

/// A chip's name: the kind for an item without a target or showing everything, else its target,
/// short: "CPU", "RAM", "Download", "RTX 4060", "/home", "nvme0n1", "wlp4s0".
#[must_use]
pub fn short_name(item: &Item, stats: &Stats) -> String {
    let gpu = |gpu: &GpuRef, auto: String| match gpu {
        GpuRef::Auto => auto,
        GpuRef::Pci(pci) => gpu_name(pci, stats),
    };
    match item {
        Item::Cpu | Item::CpuTemp => fl!("cpu"),
        Item::Ram => fl!("memory"),
        Item::Swap => fl!("swap"),
        Item::GpuUsage(g) | Item::GpuTemp(g) => gpu(g, fl!("gpu")),
        Item::Vram(g) => gpu(g, fl!("vram")),
        Item::DiskUsage(mount) => mount.clone(),
        Item::DiskRead(DiskRef::All) => fl!("read"),
        Item::DiskWrite(DiskRef::All) => fl!("write"),
        Item::DiskRead(DiskRef::Disk(id))
        | Item::DiskWrite(DiskRef::Disk(id))
        | Item::NvmeTemp(id) => disk_short(id),
        Item::Download(NetRef::All) => fl!("download-label"),
        Item::Upload(NetRef::All) => fl!("upload-label"),
        Item::Download(NetRef::Interface(name)) | Item::Upload(NetRef::Interface(name)) => {
            name.clone()
        }
    }
}

/// An item's full name, as the picker and the selection show it: "GPU usage · RTX 4060",
/// "Disk read · All disks", "CPU temperature".
#[must_use]
pub fn full_name(item: &Item, stats: &Stats) -> String {
    let kind = match item {
        Item::Cpu => fl!("item-cpu"),
        Item::CpuTemp => fl!("item-cpu-temp"),
        Item::Ram => fl!("item-ram"),
        Item::Swap => fl!("item-swap"),
        Item::GpuUsage(_) => fl!("item-gpu"),
        Item::GpuTemp(_) => fl!("item-gpu-temp"),
        Item::Vram(_) => fl!("item-vram"),
        Item::DiskUsage(_) => fl!("item-disk-usage"),
        Item::DiskRead(_) => fl!("item-disk-read"),
        Item::DiskWrite(_) => fl!("item-disk-write"),
        Item::NvmeTemp(_) => fl!("item-nvme-temp"),
        Item::Download(_) => fl!("item-download"),
        Item::Upload(_) => fl!("item-upload"),
    };
    if item.has_target() {
        fl!(
            "item-with-target",
            kind = kind,
            target = target_name(item, stats)
        )
    } else {
        kind
    }
}

/// The name of an item's target: "Auto", a GPU's short model, a mount point, "All disks", a
/// disk's model, "All interfaces", an interface.
#[must_use]
pub fn target_name(item: &Item, stats: &Stats) -> String {
    match item {
        Item::Cpu | Item::CpuTemp | Item::Ram | Item::Swap => String::new(),
        Item::GpuUsage(g) | Item::GpuTemp(g) | Item::Vram(g) => match g {
            GpuRef::Auto => fl!("target-auto"),
            GpuRef::Pci(pci) => gpu_name(pci, stats),
        },
        Item::DiskUsage(mount) => mount.clone(),
        Item::DiskRead(DiskRef::All) | Item::DiskWrite(DiskRef::All) => fl!("target-all-disks"),
        Item::DiskRead(DiskRef::Disk(id))
        | Item::DiskWrite(DiskRef::Disk(id))
        | Item::NvmeTemp(id) => disk_name(id, stats),
        Item::Download(NetRef::All) | Item::Upload(NetRef::All) => fl!("target-all-interfaces"),
        Item::Download(NetRef::Interface(name)) | Item::Upload(NetRef::Interface(name)) => {
            name.clone()
        }
    }
}

/// The short model of the GPU at `pci`, else "GPU card1", else the address.
fn gpu_name(pci: &str, stats: &Stats) -> String {
    let gpu: Option<&GpuSample> = stats.gpus.iter().find(|g| g.pci.as_deref() == Some(pci));
    match gpu {
        Some(GpuSample {
            name: Some(name), ..
        }) => short_gpu(name),
        Some(gpu) => fl!("gpu-unnamed", card = gpu.card.clone()),
        None => pci.to_owned(),
    }
}

/// A GPU's model without the maker's and the line's names: `NVIDIA GeForce RTX 4060 Laptop
/// GPU` is `RTX 4060`, `AMD Radeon 680M` is `Radeon 680M`.
#[must_use]
pub fn short_gpu(name: &str) -> String {
    let mut name = name.trim();
    for prefix in ["NVIDIA ", "GeForce ", "AMD ", "Intel(R) ", "Intel "] {
        name = name.strip_prefix(prefix).unwrap_or(name);
    }
    name.strip_suffix(" Laptop GPU").unwrap_or(name).to_owned()
}

/// A disk's model as `stats` has it now, else as remembered.
fn disk_name(id: &DiskId, stats: &Stats) -> String {
    find_disk(&stats.disks, id).map_or_else(|| id.model.clone(), |d| d.model.clone())
}

/// A disk's kernel name, else its model.
fn disk_short(id: &DiskId) -> String {
    if id.name.is_empty() {
        id.model.clone()
    } else {
        id.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(row: Row, index: usize) -> Slot {
        Slot { row, index }
    }

    fn top(index: usize) -> Slot {
        slot(Row::Top, index)
    }

    #[test]
    fn a_chip_is_selected_and_unselected_by_clicking_it() {
        let mut rows = Rows::default();
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(1)), &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        builder.change(BuilderChange::Select(top(0)), &mut rows);
        assert_eq!(builder.selected(), Some(top(0)));
        builder.change(BuilderChange::Select(top(0)), &mut rows);
        assert_eq!(builder.selected(), None);
        // No item there.
        builder.change(BuilderChange::Select(top(3)), &mut rows);
        assert_eq!(builder.selected(), None);
    }

    #[test]
    fn the_selection_follows_its_item_when_it_moves() {
        let mut rows = Rows::default();
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), &mut rows);
        builder.change(BuilderChange::Shift(top(0), true), &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        assert_eq!(rows.get(top(1)), Some(&Item::Cpu));
        builder.change(BuilderChange::ToOtherRow(top(1)), &mut rows);
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 2)));
        assert_eq!(rows.get(slot(Row::Bottom, 2)), Some(&Item::Cpu));
        // At the row's end: stays, still selected.
        builder.change(BuilderChange::Shift(slot(Row::Bottom, 2), true), &mut rows);
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 2)));
    }

    #[test]
    fn removing_an_item_keeps_the_selection_on_its_own_item() {
        let mut rows = Rows::new(vec![Item::Cpu, Item::CpuTemp, Item::Ram], vec![Item::Swap]);
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(2)), &mut rows);
        builder.change(BuilderChange::Remove(top(0)), &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        assert_eq!(rows.get(top(1)), Some(&Item::Ram));
        builder.change(BuilderChange::Remove(slot(Row::Bottom, 0)), &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        builder.change(BuilderChange::Remove(top(1)), &mut rows);
        assert_eq!(builder.selected(), None);
        assert_eq!(rows.row(Row::Top), [Item::CpuTemp]);
    }

    #[test]
    fn the_picker_adds_to_the_end_of_its_row_and_closes() {
        let mut rows = Rows::default();
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), &mut rows);
        builder.change(BuilderChange::OpenPicker(Row::Bottom), &mut rows);
        assert_eq!(
            (builder.adding(), builder.selected()),
            (Some(Row::Bottom), None)
        );
        let gpu = Item::GpuTemp(GpuRef::Pci("0000:01:00.0".into()));
        builder.change(BuilderChange::Add(Row::Bottom, gpu.clone()), &mut rows);
        assert_eq!(builder.adding(), None);
        assert_eq!(rows.get(slot(Row::Bottom, 2)), Some(&gpu));
    }

    #[test]
    fn a_full_row_opens_no_picker() {
        let mut rows = Rows::new(vec![Item::Cpu; 4], vec![]);
        let mut builder = Builder::default();
        builder.change(BuilderChange::OpenPicker(Row::Top), &mut rows);
        assert_eq!(builder.adding(), None);
        builder.change(BuilderChange::Add(Row::Top, Item::Ram), &mut rows);
        assert_eq!(rows.row(Row::Top), vec![Item::Cpu; 4]);
    }

    #[test]
    fn a_new_target_replaces_the_item_in_place_only_for_the_same_kind() {
        let mut rows = Rows::new(vec![Item::GpuUsage(GpuRef::Auto)], vec![]);
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), &mut rows);
        let nvidia = Item::GpuUsage(GpuRef::Pci("0000:01:00.0".into()));
        builder.change(BuilderChange::Retarget(top(0), nvidia.clone()), &mut rows);
        assert_eq!(rows.get(top(0)), Some(&nvidia));
        assert_eq!(builder.selected(), Some(top(0)));
        builder.change(BuilderChange::Retarget(top(0), Item::Ram), &mut rows);
        assert_eq!(rows.get(top(0)), Some(&nvidia));
    }

    #[test]
    fn gpu_models_are_shortened_to_their_line_and_number() {
        assert_eq!(short_gpu("NVIDIA GeForce RTX 4060 Laptop GPU"), "RTX 4060");
        assert_eq!(short_gpu("AMD Radeon 680M"), "Radeon 680M");
        assert_eq!(short_gpu("AMD Radeon Graphics"), "Radeon Graphics");
    }
}
