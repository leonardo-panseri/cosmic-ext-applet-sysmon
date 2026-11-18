// SPDX-License-Identifier: MPL-2.0

//! The panel button's builder (`docs/panel-builder.md`), a sub-page of the settings page: the
//! panel button's layout, and its two rows as chips, or with the one-line layout its one line.
//! Selecting a chip offers to move it, move it to the other row or remove it, and to choose its
//! GPU, mount, disk, drive or interface; **+** opens a picker of each kind of item, grouped, and
//! selects the item it adds. Every change saves at once, as the settings page's do, and shows on
//! the panel itself.

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
    DiskId, DiskRef, GpuRef, Group, Item, Layout, NetRef, PanelSettings, Row, Rows, Slot, choices,
    find_disk,
};
use crate::sampler::stats::{GpuSample, Stats};
use crate::ui::icons::{self, Glyph};

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
    /// **+** beside a row's caption; on one line, beside the line's.
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

    /// Makes `change` to `rows`, keeping the selection on the item it was on. An added item is
    /// selected, so its target can be chosen at once.
    ///
    /// With the [one-line](Layout::OneLine) layout the builder shows the rows as one line, so
    /// moves, removals and additions work on the line, and the line is then folded back into two
    /// rows of about the same length ([`Rows::fold`]): what stacking it again shows. Rows left
    /// alone on one line keep their own split.
    pub fn change(&mut self, change: BuilderChange, layout: Layout, rows: &mut Rows) {
        if layout == Layout::OneLine && self.change_line(&change, rows) {
            return;
        }
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
                self.adding = None;
                if let Some(slot) = rows.push(row, item) {
                    self.selected = Some(slot);
                }
            }
        }
    }

    /// `change` made on the rows as one line, if it is one that works on the line; `false`
    /// leaves it to [`Self::change`].
    fn change_line(&mut self, change: &BuilderChange, rows: &mut Rows) -> bool {
        let mut line = rows.line();
        let selected = self.selected.map(|s| rows.line_index(s));
        let selected = match *change {
            BuilderChange::Remove(slot) => {
                let at = rows.line_index(slot);
                if at >= line.len() {
                    return true;
                }
                line.remove(at);
                match selected {
                    Some(s) if s == at => None,
                    Some(s) if s > at => Some(s - 1),
                    other => other,
                }
            }
            BuilderChange::Shift(slot, right) => {
                let at = rows.line_index(slot);
                let to = if right {
                    at + 1
                } else {
                    match at.checked_sub(1) {
                        Some(to) => to,
                        None => return true,
                    }
                };
                if to >= line.len() {
                    return true;
                }
                line.swap(at, to);
                Some(to)
            }
            BuilderChange::OpenPicker(row) => {
                if rows.line_has_room() {
                    self.selected = None;
                    self.adding = Some(row);
                }
                return true;
            }
            BuilderChange::Add(_, ref item) => {
                self.adding = None;
                if !rows.line_has_room() {
                    return true;
                }
                line.push(item.clone());
                Some(line.len() - 1)
            }
            _ => return false,
        };
        *rows = Rows::fold(line);
        self.selected = selected.map(|s| rows.line_slot(s));
        true
    }

    /// Back to nothing selected and no picker, as when the page opens.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The builder page: the layout and the rows, then what the selection or the picker offers; in
/// two columns, those beside the rest. The panel itself shows every change, so there is no
/// preview. Scrolls beyond `max_height`.
pub fn view<'a>(
    stats: Option<&'a Stats>,
    settings: &PanelSettings,
    models: &'a SettingsModels,
    columns: Columns,
    max_height: f32,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let fallback = Stats::default();
    let stats = stats.unwrap_or(&fallback);
    let builder = models.builder();
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
    let mut main = vec![layout_choice];
    match settings.layout {
        Layout::Stacked => {
            for row in [Row::Top, Row::Bottom] {
                main.push(chips(Some(row), settings, stats, builder));
            }
        }
        Layout::OneLine => main.push(chips(None, settings, stats, builder)),
    }
    let detail = match (builder.adding(), builder.selected()) {
        (Some(row), _) => picker(row, settings.layout, stats),
        (None, Some(slot)) => match settings.rows.get(slot) {
            Some(item) => selection(slot, item, settings, stats),
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

/// A row's chips under its caption, or with `None` every item's, as one line; **+** at the
/// caption's end, so adding never moves the chips.
fn chips<'a>(
    row: Option<Row>,
    settings: &PanelSettings,
    stats: &Stats,
    builder: &Builder,
) -> Element<'a, Message> {
    let rows = &settings.rows;
    let (title, slots, room, picker_row): (String, Vec<Slot>, bool, Row) = match row {
        Some(row) => (
            match row {
                Row::Top => fl!("setting-top-row"),
                Row::Bottom => fl!("setting-bottom-row"),
            },
            (0..rows.row(row).len())
                .map(|index| Slot { row, index })
                .collect(),
            rows.has_room(row),
            row,
        ),
        // The line adds to its end, the bottom row.
        None => (
            fl!("setting-items"),
            (0..rows.line().len()).map(|i| rows.line_slot(i)).collect(),
            rows.line_has_room(),
            Row::Bottom,
        ),
    };
    let chips: Vec<Element<'a, Message>> = slots
        .into_iter()
        .filter_map(|slot| {
            let item = rows.get(slot)?;
            Some(chip(slot, item, stats, builder.selected() == Some(slot)))
        })
        .collect();
    let add = widget::tooltip(
        widget::button::icon(icon::from_name("list-add-symbolic"))
            .extra_small()
            .on_press_maybe(room.then_some(Message::Setting(SettingChange::Builder(
                BuilderChange::OpenPicker(picker_row),
            ))))
            .selected(builder.adding() == Some(picker_row)),
        widget::text::body(fl!("builder-add")),
        widget::tooltip::Position::Top,
    );
    let spacing = cosmic::theme::spacing();
    let head = widget::Row::new()
        .align_y(Alignment::Center)
        .push(popup::muted(widget::text::caption_heading(title)).width(Length::Fill))
        .push(add);
    let body: Element<'a, Message> = if chips.is_empty() {
        popup::muted(widget::text::body(fl!("builder-empty-row"))).into()
    } else {
        widget::flex_row(chips)
            .row_spacing(spacing.space_xxs)
            .column_spacing(spacing.space_xxs)
            .align_items(Alignment::Center)
            .into()
    };
    widget::Column::new()
        .spacing(spacing.space_xxxs)
        .push(head)
        .push(body)
        .into()
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

/// The selected item: its name, the moves and **Remove**, then its targets to choose from. On
/// one line it moves along the line, and has no other row to go to.
fn selection<'a>(
    slot: Slot,
    item: &Item,
    settings: &PanelSettings,
    stats: &Stats,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let rows = &settings.rows;
    let change = |change: BuilderChange| Message::Setting(SettingChange::Builder(change));
    let (at, len) = match settings.layout {
        Layout::Stacked => (slot.index, rows.row(slot.row).len()),
        Layout::OneLine => (rows.line_index(slot), rows.line().len()),
    };
    let action = |icon_name: &'static str, label: String, message: Option<Message>| {
        widget::button::standard(label)
            .leading_icon(icon::from_name(icon_name))
            .on_press_maybe(message)
            .into()
    };
    let mut actions: Vec<Element<'a, Message>> = vec![
        action(
            "go-previous-symbolic",
            fl!("builder-move-left"),
            (at > 0).then(|| change(BuilderChange::Shift(slot, false))),
        ),
        action(
            "go-next-symbolic",
            fl!("builder-move-right"),
            (at + 1 < len).then(|| change(BuilderChange::Shift(slot, true))),
        ),
    ];
    if settings.layout == Layout::Stacked {
        let (other_icon, other_label) = match slot.row {
            Row::Top => ("go-down-symbolic", fl!("builder-to-bottom-row")),
            Row::Bottom => ("go-up-symbolic", fl!("builder-to-top-row")),
        };
        actions.push(action(
            other_icon,
            other_label,
            rows.has_room(slot.row.other())
                .then(|| change(BuilderChange::ToOtherRow(slot))),
        ));
    }
    actions.push(action(
        "edit-delete-symbolic",
        fl!("builder-remove"),
        Some(change(BuilderChange::Remove(slot))),
    ));
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

/// The picker for `row`, or for the line: each kind of item once, under the group captions,
/// with the live value of its usual target ([`choices`]); and a way to close it.
fn picker<'a>(row: Row, layout: Layout, stats: &Stats) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let change = |change: BuilderChange| Message::Setting(SettingChange::Builder(change));
    let title = match (layout, row) {
        (Layout::OneLine, _) => fl!("builder-add"),
        (Layout::Stacked, Row::Top) => fl!("builder-add-top"),
        (Layout::Stacked, Row::Bottom) => fl!("builder-add-bottom"),
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
                    .push(popup::one_line(widget::text::body(kind_name(item))).width(Length::Fill))
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
    let kind = kind_name(item);
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

/// The name of an item's kind, whatever its target: "GPU usage", "Disk read".
#[must_use]
pub fn kind_name(item: &Item) -> String {
    match item {
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
        builder.change(BuilderChange::Select(top(1)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        builder.change(BuilderChange::Select(top(0)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), Some(top(0)));
        builder.change(BuilderChange::Select(top(0)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), None);
        // No item there.
        builder.change(BuilderChange::Select(top(3)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), None);
    }

    #[test]
    fn the_selection_follows_its_item_when_it_moves() {
        let mut rows = Rows::default();
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), Layout::Stacked, &mut rows);
        builder.change(
            BuilderChange::Shift(top(0), true),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.selected(), Some(top(1)));
        assert_eq!(rows.get(top(1)), Some(&Item::Cpu));
        builder.change(
            BuilderChange::ToOtherRow(top(1)),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 2)));
        assert_eq!(rows.get(slot(Row::Bottom, 2)), Some(&Item::Cpu));
        // At the row's end: stays, still selected.
        builder.change(
            BuilderChange::Shift(slot(Row::Bottom, 2), true),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 2)));
    }

    #[test]
    fn removing_an_item_keeps_the_selection_on_its_own_item() {
        let mut rows = Rows::new(vec![Item::Cpu, Item::CpuTemp, Item::Ram], vec![Item::Swap]);
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(2)), Layout::Stacked, &mut rows);
        builder.change(BuilderChange::Remove(top(0)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), Some(top(1)));
        assert_eq!(rows.get(top(1)), Some(&Item::Ram));
        builder.change(
            BuilderChange::Remove(slot(Row::Bottom, 0)),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.selected(), Some(top(1)));
        builder.change(BuilderChange::Remove(top(1)), Layout::Stacked, &mut rows);
        assert_eq!(builder.selected(), None);
        assert_eq!(rows.row(Row::Top), [Item::CpuTemp]);
    }

    #[test]
    fn the_picker_adds_to_the_end_of_its_row_and_closes() {
        let mut rows = Rows::default();
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), Layout::Stacked, &mut rows);
        builder.change(
            BuilderChange::OpenPicker(Row::Bottom),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(
            (builder.adding(), builder.selected()),
            (Some(Row::Bottom), None)
        );
        let gpu = Item::GpuTemp(GpuRef::Pci("0000:01:00.0".into()));
        builder.change(
            BuilderChange::Add(Row::Bottom, gpu.clone()),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.adding(), None);
        assert_eq!(rows.get(slot(Row::Bottom, 2)), Some(&gpu));
        // Selected, so its GPU can be chosen at once.
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 2)));
    }

    #[test]
    fn on_one_line_items_move_across_the_rows_and_the_line_folds_in_two() {
        let mut rows = Rows::new(vec![Item::Cpu, Item::CpuTemp, Item::Ram], vec![Item::Swap]);
        let mut builder = Builder::default();
        let line = Layout::OneLine;
        // Selecting changes nothing: the rows keep their own split.
        builder.change(BuilderChange::Select(top(2)), line, &mut rows);
        assert_eq!(rows.row(Row::Top).len(), 3);
        // RAM moves right, past the end of the top row: the line folds two over two.
        builder.change(BuilderChange::Shift(top(2), true), line, &mut rows);
        assert_eq!(
            rows,
            Rows::new(vec![Item::Cpu, Item::CpuTemp], vec![Item::Swap, Item::Ram])
        );
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 1)));
        // At the line's end: stays.
        builder.change(
            BuilderChange::Shift(slot(Row::Bottom, 1), true),
            line,
            &mut rows,
        );
        assert_eq!(builder.selected(), Some(slot(Row::Bottom, 1)));
        // Removing the first item keeps the selection on RAM.
        builder.change(BuilderChange::Remove(top(0)), line, &mut rows);
        assert_eq!(
            rows,
            Rows::new(vec![Item::CpuTemp, Item::Swap], vec![Item::Ram])
        );
        assert_eq!(rows.get(builder.selected().unwrap()), Some(&Item::Ram));
    }

    #[test]
    fn on_one_line_the_picker_adds_to_the_line_s_end_while_both_rows_have_room() {
        let mut rows = Rows::new(vec![Item::Cpu; 4], vec![Item::Ram; 3]);
        let mut builder = Builder::default();
        let line = Layout::OneLine;
        // The top row is full, but the line isn't.
        builder.change(BuilderChange::OpenPicker(Row::Bottom), line, &mut rows);
        assert_eq!(builder.adding(), Some(Row::Bottom));
        builder.change(BuilderChange::Add(Row::Bottom, Item::Swap), line, &mut rows);
        assert_eq!(rows.line().last(), Some(&Item::Swap));
        assert_eq!(rows.get(builder.selected().unwrap()), Some(&Item::Swap));
        // Full now.
        builder.change(BuilderChange::OpenPicker(Row::Bottom), line, &mut rows);
        assert_eq!(builder.adding(), None);
        builder.change(
            BuilderChange::Add(Row::Bottom, Item::CpuTemp),
            line,
            &mut rows,
        );
        assert_eq!(rows.line().len(), 8);
        assert!(!rows.line().contains(&Item::CpuTemp));
    }

    #[test]
    fn a_full_row_opens_no_picker() {
        let mut rows = Rows::new(vec![Item::Cpu; 4], vec![]);
        let mut builder = Builder::default();
        builder.change(
            BuilderChange::OpenPicker(Row::Top),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(builder.adding(), None);
        builder.change(
            BuilderChange::Add(Row::Top, Item::Ram),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(rows.row(Row::Top), vec![Item::Cpu; 4]);
    }

    #[test]
    fn a_new_target_replaces_the_item_in_place_only_for_the_same_kind() {
        let mut rows = Rows::new(vec![Item::GpuUsage(GpuRef::Auto)], vec![]);
        let mut builder = Builder::default();
        builder.change(BuilderChange::Select(top(0)), Layout::Stacked, &mut rows);
        let nvidia = Item::GpuUsage(GpuRef::Pci("0000:01:00.0".into()));
        builder.change(
            BuilderChange::Retarget(top(0), nvidia.clone()),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(rows.get(top(0)), Some(&nvidia));
        assert_eq!(builder.selected(), Some(top(0)));
        builder.change(
            BuilderChange::Retarget(top(0), Item::Ram),
            Layout::Stacked,
            &mut rows,
        );
        assert_eq!(rows.get(top(0)), Some(&nvidia));
    }

    #[test]
    fn gpu_models_are_shortened_to_their_line_and_number() {
        assert_eq!(short_gpu("NVIDIA GeForce RTX 4060 Laptop GPU"), "RTX 4060");
        assert_eq!(short_gpu("AMD Radeon 680M"), "Radeon 680M");
        assert_eq!(short_gpu("AMD Radeon Graphics"), "Radeon Graphics");
    }
}
