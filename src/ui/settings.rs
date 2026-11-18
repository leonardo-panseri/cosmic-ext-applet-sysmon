// SPDX-License-Identifier: MPL-2.0

//! The popup's settings page (plan §2.3): the panel button's layout and rows, the network rates,
//! the process monitor and the warning limits. Every change applies at once; the app stores it
//! and tells the sampler. Typed text is a draft until Enter or leaving the field: then it is
//! saved if valid, else dropped, and the field shows the setting again.

use std::collections::HashMap;

use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget::segmented_button::{self, Entity, SingleSelectModel, StyleSheet};
use cosmic::widget::{self, icon, segmented_control, settings};

use super::popup::{self, Page};
use crate::app::Message;
use crate::fl;
use crate::launch::Launcher;
use crate::panel::{
    DEFAULT_PROCESS_MONITOR, Layout, LimitKind, NVME_STEP_FROM, PanelSettings, RowKind, Unit,
};

/// A text field of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Limit(LimitKind),
    ProcessMonitor,
}

/// A change made on the settings page.
#[derive(Debug, Clone)]
pub enum SettingChange {
    /// A segment of the layout choice.
    Layout(Entity),
    /// A segment of the top row's choice.
    Top(Entity),
    /// A segment of the bottom row's choice.
    Bottom(Entity),
    Network(bool),
    /// A warning turned on or off.
    WarnOn(LimitKind, bool),
    /// A limit set with − or +.
    WarnAt(LimitKind, u8),
    /// Text typed into a field: kept as its draft.
    Edit(Field, String),
    /// Enter, or the field lost focus: its draft is saved if valid, else dropped.
    Commit(Field),
    /// Every draft committed, on leaving the page or closing the popup.
    CommitAll,
    /// The process monitor back to COSMIC's System Monitor.
    ResetProcessMonitor,
}

/// The longest process monitor command line taken, in bytes.
const MAX_COMMAND: usize = 1024;

/// The settings page's state beyond the settings: the segmented controls, kept in step with
/// the settings, and the fields' drafts.
pub struct SettingsModels {
    layout: SingleSelectModel,
    top: SingleSelectModel,
    bottom: SingleSelectModel,
    drafts: HashMap<Field, String>,
}

impl SettingsModels {
    /// The controls for `settings`.
    #[must_use]
    pub fn new(settings: &PanelSettings) -> Self {
        let mut models = Self {
            layout: model(&Layout::ALL, layout_label),
            top: model(&RowKind::ALL, row_label),
            bottom: model(&RowKind::ALL, row_label),
            drafts: HashMap::new(),
        };
        models.select(settings);
        models
    }

    /// Selects `settings` in every control, as after a change made elsewhere. Drafts stay.
    pub fn select(&mut self, settings: &PanelSettings) {
        select(&mut self.layout, &settings.layout);
        select(&mut self.top, &settings.top);
        select(&mut self.bottom, &settings.bottom);
    }

    /// What `field` shows: its draft, or `settings`' value.
    #[must_use]
    pub fn text(&self, field: Field, settings: &PanelSettings) -> String {
        if let Some(draft) = self.drafts.get(&field) {
            return draft.clone();
        }
        match field {
            Field::Limit(LimitKind::Nvme) => settings
                .warn_nvme
                .at
                .map_or_else(String::new, |at| at.to_string()),
            Field::Limit(kind) => settings
                .limit(kind)
                .map_or_else(String::new, |limit| limit.at.to_string()),
            Field::ProcessMonitor => settings.process_monitor.clone(),
        }
    }

    /// `settings` with `change` made, which also updates the drafts. Pass the result to
    /// [`Self::select`].
    #[must_use]
    pub fn changed(&mut self, change: SettingChange, settings: &PanelSettings) -> PanelSettings {
        let mut settings = settings.clone();
        match change {
            SettingChange::Layout(item) => {
                settings.layout = value(&self.layout, item).unwrap_or(settings.layout);
            }
            SettingChange::Top(item) => {
                settings.top = value(&self.top, item).unwrap_or(settings.top);
            }
            SettingChange::Bottom(item) => {
                settings.bottom = value(&self.bottom, item).unwrap_or(settings.bottom);
            }
            SettingChange::Network(network) => settings.network = network,
            SettingChange::WarnOn(kind, on) => match settings.limit_mut(kind) {
                Some(limit) => limit.on = on,
                None => settings.warn_nvme.on = on,
            },
            SettingChange::WarnAt(kind, at) => {
                self.drafts.remove(&Field::Limit(kind));
                set_limit(&mut settings, kind, Typed::At(at));
            }
            SettingChange::Edit(field, text) => {
                if accepts(field, &text) {
                    self.drafts.insert(field, text);
                }
            }
            SettingChange::Commit(field) => self.commit(field, &mut settings),
            SettingChange::CommitAll => {
                let fields: Vec<Field> = self.drafts.keys().copied().collect();
                for field in fields {
                    self.commit(field, &mut settings);
                }
            }
            SettingChange::ResetProcessMonitor => {
                self.drafts.remove(&Field::ProcessMonitor);
                DEFAULT_PROCESS_MONITOR.clone_into(&mut settings.process_monitor);
            }
        }
        settings.clamped()
    }

    /// Saves `field`'s draft into `settings` if it is valid, and drops it either way.
    fn commit(&mut self, field: Field, settings: &mut PanelSettings) {
        let Some(draft) = self.drafts.remove(&field) else {
            return;
        };
        match field {
            Field::Limit(kind) => {
                if let Some(at) = parse_limit(kind, &draft) {
                    set_limit(settings, kind, at);
                }
            }
            Field::ProcessMonitor => {
                let draft = draft.trim();
                if Launcher::parse(draft).is_some() {
                    settings.process_monitor = if draft.is_empty() {
                        DEFAULT_PROCESS_MONITOR.to_owned()
                    } else {
                        draft.to_owned()
                    };
                }
            }
        }
    }
}

/// A limit as typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typed {
    /// °C or %, clamped to the limit's range.
    At(u8),
    /// Nothing, for the `NVMe` drives' own rated limits.
    Rated,
}

/// Sets the limit of `kind` to `typed`. [`Typed::Rated`] leaves any limit but the `NVMe` one as
/// it is.
fn set_limit(settings: &mut PanelSettings, kind: LimitKind, typed: Typed) {
    match (settings.limit_mut(kind), typed) {
        (Some(limit), Typed::At(at)) => limit.at = at,
        (Some(_), Typed::Rated) => {}
        (None, Typed::At(at)) => settings.warn_nvme.at = Some(at),
        (None, Typed::Rated) => settings.warn_nvme.at = None,
    }
}

/// Whether `field` takes `text` as its draft: a limit only up to three digits, the process
/// monitor anything not too long.
fn accepts(field: Field, text: &str) -> bool {
    match field {
        Field::Limit(_) => text.len() <= 3 && text.chars().all(|c| c.is_ascii_digit()),
        Field::ProcessMonitor => text.len() <= MAX_COMMAND,
    }
}

/// The limit of `kind` typed as `text`; `None` when it is no number, or empty for any limit but
/// the `NVMe` one.
fn parse_limit(kind: LimitKind, text: &str) -> Option<Typed> {
    let text = text.trim();
    if text.is_empty() {
        return (kind == LimitKind::Nvme).then_some(Typed::Rated);
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Only digits: too large for a `u8` is above every range.
    let value = text.parse::<u8>().unwrap_or(u8::MAX);
    Some(Typed::At(kind.clamp(value)))
}

/// A segmented choice of `items`.
fn model<T: Copy + 'static>(items: &[T], label: fn(T) -> String) -> SingleSelectModel {
    items
        .iter()
        .fold(SingleSelectModel::builder(), |model, &item| {
            model.insert(move |entry| entry.text(label(item)).data(item))
        })
        .build()
}

fn select<T: PartialEq + 'static>(model: &mut SingleSelectModel, value: &T) {
    let item = model
        .iter()
        .find(|&item| model.data::<T>(item) == Some(value));
    if let Some(item) = item {
        model.activate(item);
    }
}

fn value<T: Copy + 'static>(model: &SingleSelectModel, item: Entity) -> Option<T> {
    model.data::<T>(item).copied()
}

fn layout_label(layout: Layout) -> String {
    match layout {
        Layout::Stacked => fl!("layout-stacked"),
        Layout::OneLine => fl!("layout-one-line"),
        Layout::Compact => fl!("layout-compact"),
    }
}

fn row_label(kind: RowKind) -> String {
    match kind {
        RowKind::Cpu => fl!("cpu"),
        RowKind::Memory => fl!("memory"),
        RowKind::Gpu => fl!("gpu"),
        RowKind::Disk => fl!("disk"),
        RowKind::CpuTemp => fl!("row-cpu-temp"),
    }
}

fn limit_label(kind: LimitKind) -> String {
    match kind {
        LimitKind::CpuTemp => fl!("setting-warn-cpu-temp"),
        LimitKind::GpuTemp => fl!("setting-warn-gpu-temp"),
        LimitKind::Memory => fl!("setting-warn-memory"),
        LimitKind::Swap => fl!("setting-warn-swap"),
        LimitKind::Disk => fl!("setting-warn-disk"),
        LimitKind::Vram => fl!("setting-warn-vram"),
        LimitKind::Nvme => fl!("setting-warn-nvme"),
    }
}

/// The segmented control's look without the check mark on the active segment, which leaves too
/// little room for "CPU °C" when five segments share the popup's width. The active segment
/// still stands out by its colours.
fn control_without_check(theme: &cosmic::Theme) -> segmented_button::Appearance {
    theme.horizontal(&cosmic::theme::SegmentedButton::Control)
}

/// The settings page: the panel button's choices, the network rates, the process monitor and the
/// warning limits, scrolling above a way back to the stats page.
pub fn view<'a>(models: &'a SettingsModels, settings: &PanelSettings) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let choice =
        |title: String, model: &'a SingleSelectModel, change: fn(Entity) -> SettingChange| {
            popup::section(
                title,
                vec![
                    segmented_control::horizontal(model)
                        .on_activate(move |item| Message::Setting(change(item)))
                        .style(cosmic::theme::SegmentedButton::Custom(Box::new(
                            control_without_check,
                        )))
                        // The segments share the width equally; less padding leaves the text
                        // room before it is cut.
                        .button_padding([spacing.space_xxs, 0, spacing.space_xxs, 0])
                        .width(Length::Fill)
                        .into(),
                ],
            )
        };
    let network = settings::item::builder(fl!("setting-network"))
        .description(fl!("setting-network-description"))
        .control(
            widget::toggler(settings.network)
                .on_toggle(|on| Message::Setting(SettingChange::Network(on))),
        );
    let limits = LimitKind::ALL
        .into_iter()
        .map(|kind| limit_item(models, settings, kind))
        .collect();
    let page = widget::Column::new()
        .spacing(spacing.space_s)
        .push(choice(
            fl!("setting-layout"),
            &models.layout,
            SettingChange::Layout,
        ))
        .push(choice(
            fl!("setting-top-row"),
            &models.top,
            SettingChange::Top,
        ))
        .push(choice(
            fl!("setting-bottom-row"),
            &models.bottom,
            SettingChange::Bottom,
        ))
        .push(widget::divider::horizontal::default())
        .push(network)
        .push(widget::divider::horizontal::default())
        .push(process_monitor(models, settings))
        .push(widget::divider::horizontal::default())
        .push(popup::section(fl!("setting-warn"), limits))
        // As on the stats page, the scroll bar runs in the popup's side padding.
        .padding([0, spacing.space_s]);
    let padded =
        |content: Element<'a, Message>| widget::container(content).padding([0, spacing.space_s]);
    widget::Column::new()
        .spacing(spacing.space_s)
        .align_x(Alignment::Start)
        .push(padded(widget::text::title4(fl!("settings")).into()))
        .push(widget::container(widget::scrollable(page)).max_height(popup::PAGE_MAX_HEIGHT))
        .push(padded(popup::footer_button(
            "go-previous-symbolic",
            fl!("back"),
            Message::ShowPage(Page::Stats),
        )))
        .into()
}

/// The process monitor's field, under its title and hint, with a reset to the default.
fn process_monitor<'a>(models: &SettingsModels, settings: &PanelSettings) -> Element<'a, Message> {
    let field = Field::ProcessMonitor;
    let text = models.text(field, settings);
    let is_default = text == DEFAULT_PROCESS_MONITOR;
    let reset = widget::tooltip(
        widget::button::icon(icon::from_name("edit-undo-symbolic")).on_press_maybe(
            (!is_default).then_some(Message::Setting(SettingChange::ResetProcessMonitor)),
        ),
        widget::text::body(fl!("setting-process-monitor-reset")),
        widget::tooltip::Position::Left,
    );
    let input = widget::text_input(DEFAULT_PROCESS_MONITOR, text)
        .on_input(move |text| Message::Setting(SettingChange::Edit(field, text)))
        .on_submit(move |_| Message::Setting(SettingChange::Commit(field)))
        .on_unfocus(Message::Setting(SettingChange::Commit(field)))
        .width(Length::Fill);
    widget::Column::new()
        .spacing(spacing_xxs())
        .push(
            settings::item::builder(fl!("setting-process-monitor"))
                .description(fl!("setting-process-monitor-hint"))
                .control(reset),
        )
        .push(input)
        .into()
}

fn spacing_xxs() -> u16 {
    cosmic::theme::spacing().space_xxs
}

/// A warning's row: its name, its limit, and its switch.
fn limit_item<'a>(
    models: &SettingsModels,
    settings: &PanelSettings,
    kind: LimitKind,
) -> Element<'a, Message> {
    let on = settings.is_on(kind);
    let item = settings::item::builder(limit_label(kind));
    let item = if kind == LimitKind::Nvme {
        item.description(fl!("setting-warn-nvme-description"))
    } else if kind == LimitKind::Disk {
        item.description(fl!("setting-warn-disk-description"))
    } else {
        item
    };
    item.control(
        widget::Row::new()
            .spacing(spacing_xxs())
            .align_y(Alignment::Center)
            .push(limit_input(models, settings, kind, on))
            .push(
                widget::toggler(on)
                    .on_toggle(move |on| Message::Setting(SettingChange::WarnOn(kind, on))),
            ),
    )
    .into()
}

/// The width of a limit's text field: three digits and the unit.
const LIMIT_WIDTH: f32 = 80.0;

/// A limit: − and + around a field taking digits, with its unit. All three are off while the
/// warning is.
fn limit_input<'a>(
    models: &SettingsModels,
    settings: &PanelSettings,
    kind: LimitKind,
    on: bool,
) -> Element<'a, Message> {
    let field = Field::Limit(kind);
    let current = match settings.limit(kind) {
        Some(limit) => limit.at,
        None => settings.warn_nvme.at.unwrap_or(NVME_STEP_FROM),
    };
    let range = kind.range();
    let step = |up: bool| {
        let next = if up {
            current.checked_add(1)
        } else {
            current.checked_sub(1)
        };
        next.filter(|next| on && range.contains(next))
            .map(|next| Message::Setting(SettingChange::WarnAt(kind, next)))
    };
    let unit = match kind.unit() {
        Unit::Celsius => "°C",
        Unit::Percent => "%",
    };
    let placeholder = if kind == LimitKind::Nvme {
        fl!("setting-limit-rated")
    } else {
        String::new()
    };
    let mut input = widget::text_input(placeholder, models.text(field, settings))
        .width(Length::Fixed(LIMIT_WIDTH))
        .trailing_icon(widget::text::body(unit).into());
    if on {
        input = input
            .on_input(move |text| Message::Setting(SettingChange::Edit(field, text)))
            .on_submit(move |_| Message::Setting(SettingChange::Commit(field)))
            .on_unfocus(Message::Setting(SettingChange::Commit(field)));
    }
    let button = |name: &'static str, message: Option<Message>| {
        widget::button::icon(icon::from_name(name))
            .extra_small()
            .on_press_maybe(message)
    };
    widget::Row::new()
        .align_y(Alignment::Center)
        .push(button("list-remove-symbolic", step(false)))
        .push(input)
        .push(button("list-add-symbolic", step(true)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{Limit, NvmeLimit};

    /// The entity showing `value` in `model`.
    fn entity<T: PartialEq + 'static>(model: &SingleSelectModel, value: &T) -> Entity {
        model
            .iter()
            .find(|&item| model.data::<T>(item) == Some(value))
            .unwrap()
    }

    #[test]
    fn the_controls_start_on_the_settings() {
        let settings = PanelSettings {
            layout: Layout::Compact,
            bottom: RowKind::Disk,
            ..PanelSettings::default()
        };
        let models = SettingsModels::new(&settings);
        assert_eq!(
            value(&models.layout, models.layout.active()),
            Some(Layout::Compact)
        );
        assert_eq!(value(&models.top, models.top.active()), Some(RowKind::Cpu));
        assert_eq!(
            value(&models.bottom, models.bottom.active()),
            Some(RowKind::Disk)
        );
    }

    #[test]
    fn each_control_changes_its_setting() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let one_line = entity(&models.layout, &Layout::OneLine);
        let settings = models.changed(SettingChange::Layout(one_line), &defaults);
        assert_eq!(settings.layout, Layout::OneLine);
        models.select(&settings);
        assert_eq!(models.layout.active(), one_line);

        let gpu = entity(&models.top, &RowKind::Gpu);
        let settings = models.changed(SettingChange::Top(gpu), &settings);
        let temp = entity(&models.bottom, &RowKind::CpuTemp);
        let settings = models.changed(SettingChange::Bottom(temp), &settings);
        let settings = models.changed(SettingChange::Network(false), &settings);
        let settings = models.changed(SettingChange::WarnAt(LimitKind::CpuTemp, 70), &settings);
        let settings = models.changed(SettingChange::WarnOn(LimitKind::Swap, true), &settings);
        let settings = models.changed(SettingChange::WarnOn(LimitKind::Nvme, false), &settings);
        let settings = models.changed(SettingChange::WarnAt(LimitKind::Nvme, 79), &settings);
        assert_eq!(
            settings,
            PanelSettings {
                layout: Layout::OneLine,
                top: RowKind::Gpu,
                bottom: RowKind::CpuTemp,
                network: false,
                warn_cpu_temp: Limit::on(70),
                warn_swap: Limit::on(50),
                warn_nvme: NvmeLimit {
                    on: false,
                    at: Some(79)
                },
                ..PanelSettings::default()
            }
        );
    }

    /// `text` typed into `field`, then Enter.
    fn typed(
        models: &mut SettingsModels,
        settings: &PanelSettings,
        field: Field,
        text: &str,
    ) -> PanelSettings {
        let settings = models.changed(SettingChange::Edit(field, text.into()), settings);
        models.changed(SettingChange::Commit(field), &settings)
    }

    #[test]
    fn a_typed_limit_is_saved_on_enter_and_clamped() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let memory = Field::Limit(LimitKind::Memory);
        // While typing, only the draft changes.
        let settings = models.changed(SettingChange::Edit(memory, "7".into()), &defaults);
        assert_eq!(settings, defaults);
        assert_eq!(models.text(memory, &settings), "7");
        let settings = models.changed(SettingChange::Edit(memory, "75".into()), &settings);
        let settings = models.changed(SettingChange::Commit(memory), &settings);
        assert_eq!(settings.warn_memory, Limit::on(75));
        assert_eq!(models.text(memory, &settings), "75");

        let cpu = Field::Limit(LimitKind::CpuTemp);
        assert_eq!(
            typed(&mut models, &settings, cpu, "999").warn_cpu_temp.at,
            110
        );
        assert_eq!(typed(&mut models, &settings, cpu, "5").warn_cpu_temp.at, 50);
    }

    #[test]
    fn only_digits_are_typed_and_an_empty_limit_is_not_saved() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let disk = Field::Limit(LimitKind::Disk);
        let settings = models.changed(SettingChange::Edit(disk, "8".into()), &defaults);
        let settings = models.changed(SettingChange::Edit(disk, "8x".into()), &settings);
        assert_eq!(models.text(disk, &settings), "8");
        let settings = models.changed(SettingChange::Edit(disk, "8000".into()), &settings);
        assert_eq!(models.text(disk, &settings), "8");
        // Emptied, then the field left: nothing saved, the setting shown again.
        let settings = models.changed(SettingChange::Edit(disk, String::new()), &settings);
        let settings = models.changed(SettingChange::Commit(disk), &settings);
        assert_eq!(settings, defaults);
        assert_eq!(models.text(disk, &settings), "90");
    }

    #[test]
    fn an_empty_nvme_limit_is_each_drive_s_rated_one() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let nvme = Field::Limit(LimitKind::Nvme);
        assert_eq!(models.text(nvme, &defaults), "");
        let settings = typed(&mut models, &defaults, nvme, "72");
        assert_eq!(settings.warn_nvme.at, Some(72));
        assert_eq!(models.text(nvme, &settings), "72");
        let settings = typed(&mut models, &settings, nvme, "");
        assert_eq!(settings.warn_nvme, NvmeLimit::default());
    }

    #[test]
    fn leaving_the_page_saves_every_valid_draft() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let mut settings = defaults.clone();
        for (field, text) in [
            (Field::Limit(LimitKind::Vram), "95"),
            (Field::Limit(LimitKind::Swap), ""),
            (Field::ProcessMonitor, "missioncenter"),
        ] {
            settings = models.changed(SettingChange::Edit(field, text.into()), &settings);
        }
        let settings = models.changed(SettingChange::CommitAll, &settings);
        assert_eq!(settings.warn_vram, Limit::off(95));
        assert_eq!(settings.warn_swap, defaults.warn_swap);
        assert_eq!(settings.process_monitor, "missioncenter");
        assert!(models.drafts.is_empty());
    }

    #[test]
    fn a_step_replaces_the_draft() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let gpu = Field::Limit(LimitKind::GpuTemp);
        let settings = models.changed(SettingChange::Edit(gpu, "7".into()), &defaults);
        let settings = models.changed(SettingChange::WarnAt(LimitKind::GpuTemp, 86), &settings);
        assert_eq!(settings.warn_gpu_temp, Limit::on(86));
        assert_eq!(models.text(gpu, &settings), "86");
    }

    #[test]
    fn the_process_monitor_takes_a_valid_id_or_command_and_resets() {
        let defaults = PanelSettings::default();
        let mut models = SettingsModels::new(&defaults);
        let field = Field::ProcessMonitor;
        let settings = typed(&mut models, &defaults, field, "  kitty -e btop ");
        assert_eq!(settings.process_monitor, "kitty -e btop");
        // Unbalanced quotes: not saved, the setting shown again.
        let broken = typed(&mut models, &settings, field, "kitty -e 'btop");
        assert_eq!(broken.process_monitor, "kitty -e btop");
        assert_eq!(models.text(field, &broken), "kitty -e btop");
        // Emptied: the default.
        let emptied = typed(&mut models, &settings, field, "");
        assert_eq!(emptied.process_monitor, DEFAULT_PROCESS_MONITOR);
        let reset = models.changed(SettingChange::ResetProcessMonitor, &settings);
        assert_eq!(reset.process_monitor, DEFAULT_PROCESS_MONITOR);
    }

    #[test]
    fn a_commit_without_a_draft_changes_nothing() {
        // libcosmic sends the unfocus message of every field on any click outside it.
        let settings = PanelSettings {
            process_monitor: "missioncenter".into(),
            ..PanelSettings::default()
        };
        let mut models = SettingsModels::new(&settings);
        for field in [Field::ProcessMonitor, Field::Limit(LimitKind::Nvme)] {
            assert_eq!(
                models.changed(SettingChange::Commit(field), &settings),
                settings
            );
        }
    }
}
