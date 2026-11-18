// SPDX-License-Identifier: MPL-2.0

//! The applet: the panel button, fed by the sampler thread, and its popup (plan §2).

use std::sync::Arc;

use cosmic::iced::futures::{SinkExt, Stream, StreamExt};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::window::Id;
use cosmic::iced::{Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget;

use crate::panel::{PanelSettings, PanelSummary};
use crate::sampler::stats::Stats;
use crate::sampler::{self, Control, SamplerHandle, Update};
use crate::ui::panel_button::{self, panel_button};
use crate::ui::popup::{self, Page};
use crate::ui::settings::{SettingChange, SettingsModels};
use crate::{config, launch, redraws, upower};

/// The applet's state.
pub struct AppModel {
    /// Application state managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup's window, while it is open.
    popup: Option<Id>,
    /// The popup's page.
    page: Page,
    /// The panel button's settings, as stored.
    settings: PanelSettings,
    /// The stored settings; `None` when the config can't be opened, so changes last only until
    /// a restart.
    config: Option<cosmic::cosmic_config::Config>,
    /// The settings page's segmented controls, kept in step with `settings`.
    models: SettingsModels,
    /// The sampler's control channel, once it has started.
    sampler: Option<SamplerHandle>,
    /// The panel orientation last sent to the sampler: `true` for horizontal.
    sent_horizontal: Option<bool>,
    /// What the panel button shows, including the active warnings; `None` before the first sample.
    panel: Option<PanelSummary>,
    /// The last sample with its history, for the popup.
    stats: Option<Arc<Stats>>,
    /// When the last sample arrived, for the `SYSMON_DEBUG` redraw counts.
    last_sample: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A left click on the panel button.
    TogglePopup,
    /// A right click on the panel button, or the popup's **Process monitor** button.
    OpenProcessMonitor,
    /// One of the popup's buttons switched pages.
    ShowPage(Page),
    /// The compositor closed the popup, for example after a click outside it.
    PopupClosed(Id),
    /// The sampler thread started.
    SamplerStarted(SamplerHandle),
    /// The sampler has something new to show.
    Sampled(Update),
    /// A change on the settings page.
    Setting(SettingChange),
    /// The stored settings changed, for example from the applet on another panel.
    ConfigChanged(PanelSettings),
    /// The panel button's tooltip opens or closes its surface.
    Surface(cosmic::surface::Action<Message>),
}

impl AppModel {
    fn popup_changed(&self) {
        if let Some(sampler) = &self.sampler {
            sampler.send(Control::PopupOpen(self.popup.is_some()));
        }
    }

    /// Tells the sampler the panel's orientation if it hasn't been told yet or it changed: a
    /// vertical panel shows only the top row, so the sampler builds its summaries for that (D6)
    /// and leaves out NVML for a hidden GPU row (D2).
    fn sync_orientation(&mut self) {
        let horizontal = self.core.applet.is_horizontal();
        if let Some(sampler) = &self.sampler
            && self.sent_horizontal != Some(horizontal)
        {
            sampler.send(Control::Horizontal(horizontal));
            self.sent_horizontal = Some(horizontal);
        }
    }

    /// Makes `change` on the settings page: stores the settings it changes, if any, and applies
    /// them.
    fn change_setting(&mut self, change: SettingChange) {
        let settings = self.models.changed(change, &self.settings);
        if settings == self.settings {
            return;
        }
        if let Some(config) = &self.config {
            let mut stored = self.settings.clone();
            config::store(config, &mut stored, &settings);
        }
        config::log("changed", &settings);
        self.apply_settings(settings);
    }

    /// Closes the popup, saving what was typed on the settings page.
    fn close_popup(&mut self, id: Id) -> Task<cosmic::Action<Message>> {
        self.change_setting(SettingChange::CommitAll);
        self.popup_changed();
        destroy_popup(id)
    }

    /// Shows `settings` and tells the sampler, which sends the panel button's new summary at
    /// once if it changes.
    fn apply_settings(&mut self, settings: PanelSettings) {
        if settings == self.settings {
            return;
        }
        self.models.select(&settings);
        if let Some(sampler) = &self.sampler {
            sampler.send(Control::Settings(settings.clone()));
        }
        self.settings = settings;
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;

    type Flags = ();

    type Message = Message;

    const APP_ID: &'static str = "io.github.leonardo_panseri.cosmic-ext-applet-sysmon";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let (config, settings) = config::load(Self::APP_ID);
        config::log("loaded", &settings);
        let app = AppModel {
            core,
            popup: None,
            page: Page::Stats,
            models: SettingsModels::new(&settings),
            settings,
            config,
            sampler: None,
            sent_horizontal: None,
            panel: None,
            stats: None,
            last_sample: None,
        };
        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        let config = self
            .core
            .watch_config::<PanelSettings>(Self::APP_ID)
            .map(|update| Message::ConfigChanged(update.config));
        Subscription::batch([Subscription::run(sampler_messages), config])
    }

    fn view(&self) -> Element<'_, Self::Message> {
        redraws::count(redraws::Event::View);
        let applet = &self.core.applet;
        // The button takes only left clicks, so a right click reaches the mouse area. The mouse
        // area lays out as its content, so it changes neither the size nor the hover highlight.
        let button = widget::mouse_area(panel_button(applet, &self.settings, self.panel.as_ref()))
            .on_right_press(Message::OpenProcessMonitor);
        // The tooltip lists the warnings. It only watches the cursor and passes every event on,
        // so both clicks still reach the button and the mouse area. It is always there, only
        // without a surface to show when there is nothing to say, so the widget tree keeps its
        // shape when warnings come and go, and an open tooltip still closes on leaving.
        let tooltip = panel_button::tooltip(self.panel.as_ref());
        let silent = self.popup.is_some() || tooltip.is_empty();
        let button = applet.applet_tooltip(button, tooltip, silent, Message::Surface, None);
        // Without the autosize wrapper the panel clips the button to icon size.
        applet.autosize_window(button).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        redraws::count(redraws::Event::ViewWindow);
        let content = popup::view(
            self.stats.as_deref(),
            &self.settings,
            &self.models,
            self.page,
            popup::PAGE_MAX_HEIGHT,
        );
        self.core.applet.popup_container(content).into()
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        redraws::count(redraws::Event::Update);
        self.sync_orientation();
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return self.close_popup(id);
                }
                let Some(main) = self.core.main_window_id() else {
                    return Task::none();
                };
                let new_id = Id::unique();
                self.popup = Some(new_id);
                self.page = Page::Stats;
                self.popup_changed();
                let mut popup_settings = self
                    .core
                    .applet
                    .get_popup_settings(main, new_id, None, None, None);
                popup_settings.positioner.size_limits = Limits::NONE
                    .max_width(372.0)
                    .min_width(300.0)
                    .min_height(60.0)
                    .max_height(1080.0);
                return get_popup(popup_settings);
            }
            Message::OpenProcessMonitor => {
                launch::process_monitor(&self.settings.process_monitor);
                if let Some(id) = self.popup.take() {
                    return self.close_popup(id);
                }
            }
            Message::ShowPage(page) => {
                self.change_setting(SettingChange::CommitAll);
                self.page = page;
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                    self.change_setting(SettingChange::CommitAll);
                    self.popup_changed();
                }
            }
            Message::SamplerStarted(handle) => {
                // The popup may have opened, or the settings changed, before the sampler
                // started. It started with the stored settings; a change since is sent here.
                handle.send(Control::Settings(self.settings.clone()));
                self.sampler = Some(handle);
                self.sent_horizontal = None;
                self.sync_orientation();
                self.popup_changed();
            }
            Message::Sampled(update) => {
                redraws::log_on_sample(&mut self.last_sample);
                self.panel = Some(update.panel);
                self.stats = Some(update.stats);
            }
            Message::Setting(change) => self.change_setting(change),
            Message::ConfigChanged(settings) => {
                // The watch knows only the current keys: the old limits are read here.
                let settings = match &self.config {
                    Some(config) => config::settle(config, settings),
                    None => settings.clamped(),
                };
                if settings != self.settings {
                    config::log("changed elsewhere", &settings);
                }
                self.apply_settings(settings);
            }
            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Surface(action));
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

/// Starts the sampler thread and forwards its updates as messages; meanwhile watches `UPower` and
/// tells the sampler when the machine goes on or off battery, without going through `update()`.
pub(crate) fn sampler_messages() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut output| {
        // The stored settings as `init` read them; a change since follows as a control message.
        let (_, settings) = config::load(<AppModel as cosmic::Application>::APP_ID);
        let (sampler, mut updates) = sampler::spawn(settings);
        if output
            .send(Message::SamplerStarted(sampler.clone()))
            .await
            .is_err()
        {
            return;
        }
        let forward = async {
            while let Some(update) = updates.next().await {
                if output.send(Message::Sampled(update)).await.is_err() {
                    break;
                }
            }
        };
        let battery = upower::watch_on_battery(|on_battery| {
            sampler.send(Control::OnBattery(on_battery));
        });
        // The battery watch may end early (no UPower); the forwarding never should.
        cosmic::iced::futures::future::join(forward, battery).await;
    })
}
