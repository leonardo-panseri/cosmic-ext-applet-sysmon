// SPDX-License-Identifier: MPL-2.0

//! The popup's settings page (plan §2.3): the panel button's layout and rows, the network rates
//! and the warning limits. Every change applies at once; the app stores it and tells the
//! sampler.

use cosmic::iced::{Alignment, Length};
use cosmic::prelude::*;
use cosmic::widget::segmented_button::{self, Entity, SingleSelectModel, StyleSheet};
use cosmic::widget::{self, segmented_control, settings, spin_button};

use super::popup::{self, Page};
use crate::app::Message;
use crate::fl;
use crate::panel::{Layout, PanelSettings, RowKind, WARN_TEMP_RANGE, WARN_USAGE_RANGE, format};

/// A change made on the settings page.
#[derive(Debug, Clone, Copy)]
pub enum SettingChange {
    /// A segment of the layout choice.
    Layout(Entity),
    /// A segment of the top row's choice.
    Top(Entity),
    /// A segment of the bottom row's choice.
    Bottom(Entity),
    Network(bool),
    WarnTemp(u8),
    WarnUsage(u8),
}

/// The segmented controls' state, kept in step with the settings.
pub struct SettingsModels {
    layout: SingleSelectModel,
    top: SingleSelectModel,
    bottom: SingleSelectModel,
}

impl SettingsModels {
    /// The controls for `settings`.
    #[must_use]
    pub fn new(settings: &PanelSettings) -> Self {
        let mut models = Self {
            layout: model(&Layout::ALL, layout_label),
            top: model(&RowKind::ALL, row_label),
            bottom: model(&RowKind::ALL, row_label),
        };
        models.select(settings);
        models
    }

    /// Selects `settings` in every control, as after a change made elsewhere.
    pub fn select(&mut self, settings: &PanelSettings) {
        select(&mut self.layout, &settings.layout);
        select(&mut self.top, &settings.top);
        select(&mut self.bottom, &settings.bottom);
    }

    /// `settings` with `change` made. Pass the result to [`Self::select`].
    #[must_use]
    pub fn changed(&self, change: SettingChange, settings: &PanelSettings) -> PanelSettings {
        let settings = settings.clone();
        match change {
            SettingChange::Layout(item) => PanelSettings {
                layout: value(&self.layout, item).unwrap_or(settings.layout),
                ..settings
            },
            SettingChange::Top(item) => PanelSettings {
                top: value(&self.top, item).unwrap_or(settings.top),
                ..settings
            },
            SettingChange::Bottom(item) => PanelSettings {
                bottom: value(&self.bottom, item).unwrap_or(settings.bottom),
                ..settings
            },
            SettingChange::Network(network) => PanelSettings {
                network,
                ..settings
            },
            SettingChange::WarnTemp(warn_temp) => PanelSettings {
                warn_temp,
                ..settings
            },
            SettingChange::WarnUsage(warn_usage) => PanelSettings {
                warn_usage,
                ..settings
            },
        }
        .clamped()
    }
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

/// The segmented control's look without the check mark on the active segment, which leaves too
/// little room for "CPU °C" when five segments share the popup's width. The active segment
/// still stands out by its colours.
fn control_without_check(theme: &cosmic::Theme) -> segmented_button::Appearance {
    theme.horizontal(&cosmic::theme::SegmentedButton::Control)
}

/// The settings page: the panel button's choices, the network rates, the warning limits, then
/// a way back to the stats page.
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
    let choices = widget::Column::new()
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
        ));
    let network = settings::item::builder(fl!("setting-network"))
        .description(fl!("setting-network-description"))
        .control(
            widget::toggler(settings.network)
                .on_toggle(|on| Message::Setting(SettingChange::Network(on))),
        );
    let warn_temp = settings::item::builder(fl!("setting-warn-temp"))
        .description(fl!("setting-warn-temp-description"))
        .control(spin_button(
            format::celsius(f32::from(settings.warn_temp)),
            settings.warn_temp,
            1,
            *WARN_TEMP_RANGE.start(),
            *WARN_TEMP_RANGE.end(),
            |value| Message::Setting(SettingChange::WarnTemp(value)),
        ));
    let warn_usage = settings::item::builder(fl!("setting-warn-usage"))
        .description(fl!("setting-warn-usage-description"))
        .control(spin_button(
            format::percent(f32::from(settings.warn_usage)),
            settings.warn_usage,
            1,
            *WARN_USAGE_RANGE.start(),
            *WARN_USAGE_RANGE.end(),
            |value| Message::Setting(SettingChange::WarnUsage(value)),
        ));
    let limits = popup::section(
        fl!("setting-warn"),
        vec![warn_temp.into(), warn_usage.into()],
    );
    widget::Column::new()
        .spacing(spacing.space_s)
        .align_x(Alignment::Start)
        .push(widget::text::title4(fl!("settings")))
        .push(choices)
        .push(widget::divider::horizontal::default())
        .push(network)
        .push(widget::divider::horizontal::default())
        .push(limits)
        .push(popup::footer_button(
            "go-previous-symbolic",
            fl!("back"),
            Message::ShowPage(Page::Stats),
        ))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let settings = models.changed(SettingChange::WarnTemp(70), &settings);
        let settings = models.changed(SettingChange::WarnUsage(95), &settings);
        assert_eq!(
            settings,
            PanelSettings {
                layout: Layout::OneLine,
                top: RowKind::Gpu,
                bottom: RowKind::CpuTemp,
                network: false,
                warn_temp: 70,
                warn_usage: 95,
            }
        );
    }

    #[test]
    fn limits_stay_in_their_ranges() {
        let models = SettingsModels::new(&PanelSettings::default());
        let settings = models.changed(SettingChange::WarnTemp(5), &PanelSettings::default());
        assert_eq!(settings.warn_temp, *WARN_TEMP_RANGE.start());
        let settings = models.changed(SettingChange::WarnUsage(101), &settings);
        assert_eq!(settings.warn_usage, *WARN_USAGE_RANGE.end());
    }
}
