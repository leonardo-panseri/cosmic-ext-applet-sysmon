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
use crate::ui::panel_button::panel_button;
use crate::ui::popup::{self, Page};
use crate::{launch, upower};

/// The applet's state.
pub struct AppModel {
    /// Application state managed by the COSMIC runtime.
    core: cosmic::Core,
    /// The popup's window, while it is open.
    popup: Option<Id>,
    /// The popup's page.
    page: Page,
    /// The panel button's settings; loaded from cosmic-config in P5.
    settings: PanelSettings,
    /// The sampler's control channel, once it has started.
    sampler: Option<SamplerHandle>,
    /// What the panel button shows, including the active warnings; `None` before the first sample.
    panel: Option<PanelSummary>,
    /// The last sample with its history, for the popup.
    stats: Option<Arc<Stats>>,
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
}

impl AppModel {
    fn popup_changed(&self) {
        if let Some(sampler) = &self.sampler {
            sampler.send(Control::PopupOpen(self.popup.is_some()));
        }
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
        let app = AppModel {
            core,
            popup: None,
            page: Page::Stats,
            settings: PanelSettings::default(),
            sampler: None,
            panel: None,
            stats: None,
        };
        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run(sampler_messages)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let applet = &self.core.applet;
        // The button takes only left clicks, so a right click reaches the mouse area. The mouse
        // area lays out as its content, so it changes neither the size nor the hover highlight.
        let button = widget::mouse_area(panel_button(applet, &self.settings, self.panel.as_ref()))
            .on_right_press(Message::OpenProcessMonitor);
        // Without the autosize wrapper the panel clips the button to icon size.
        applet.autosize_window(button).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let content = popup::view(
            self.stats.as_deref(),
            &self.settings,
            self.page,
            popup::PAGE_MAX_HEIGHT,
        );
        self.core.applet.popup_container(content).into()
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    self.popup_changed();
                    return destroy_popup(id);
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
                launch::process_monitor();
                if let Some(id) = self.popup.take() {
                    self.popup_changed();
                    return destroy_popup(id);
                }
            }
            Message::ShowPage(page) => self.page = page,
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                    self.popup_changed();
                }
            }
            Message::SamplerStarted(handle) => {
                self.sampler = Some(handle);
                // The popup may have opened before the sampler started.
                self.popup_changed();
            }
            Message::Sampled(update) => {
                self.panel = Some(update.panel);
                self.stats = Some(update.stats);
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
fn sampler_messages() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(1, async |mut output| {
        // P5 restarts the subscription with the configured settings.
        let (sampler, mut updates) = sampler::spawn(PanelSettings::default());
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
