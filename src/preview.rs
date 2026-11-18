// SPDX-License-Identifier: MPL-2.0

//! Debug builds only: `SYSMON_PREVIEW=1 cargo run` shows the panel button and the popup in an
//! ordinary window, fed with fixed, made-up [`Stats`] ([`fixture`]): two GPUs (a suspended
//! NVIDIA card and an AMD iGPU), two network interfaces, two `NVMe` drives with four filesystems
//! (one on LUKS), full histories and the min/max since start. Nothing is read from the system,
//! so no GPU is woken. The **Settings** button switches pages; settings apply to the preview
//! only; **Process monitor** only prints.
//!
//! `SYSMON_PREVIEW_SCREENSHOT=<file>` instead draws a mock top panel over a backdrop, in COSMIC's
//! default dark theme, saves the window's pixels to `<file>` and exits. `SYSMON_PREVIEW_HEIGHT`
//! sets the window's height in that mode, which stands for the output's: the popup's room is
//! measured from it as on the panel. `SYSMON_PREVIEW_COLUMNS=1|2|auto` (default `auto`) sets the
//! popup's layout; outside screenshot mode `auto` measures the real output, from the same output
//! events as the applet. `SYSMON_PREVIEW_SETTINGS=1` starts on the settings page,
//! `SYSMON_PREVIEW_BUILDER=1` on the panel button's builder (`=selected` with the third item of
//! the top row selected, `=picker` with the bottom row's picker open). `SYSMON_PREVIEW_LAYOUT=stacked|one-line` picks the panel
//! button's layout, `SYSMON_PREVIEW_ROWS=custom` four items over three (with GPU and `NVMe`
//! items) instead of the default rows, and `COSMIC_PANEL_SIZE` (`XS`…`XL`) and
//! `COSMIC_PANEL_ANCHOR` (`Left` for a vertical panel) its panel, as libcosmic reads them.
//! `SYSMON_PREVIEW_PANEL=1` shows only the panel, along the top of the window or down its left
//! side for a vertical panel, in a small window. The file is `RGBA`, the width and height as
//! little-endian `u32`s, then the pixels; `tools/screenshots.py` turns it into a PNG.
//!
//! `SYSMON_PREVIEW_LIVE=1` instead feeds the preview from the real sampler, as the panel does
//! (closed tier, or open tier with `SYSMON_DEBUG_OPEN`), to measure redraws and wakeups in an
//! ordinary window; `SYSMON_DEBUG` then logs the redraw counts of each sample. With
//! `SYSMON_PREVIEW_SCREENSHOT` too, the screenshot waits [`LIVE_SHOT_DELAY`], for the rates of a
//! second sample.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cosmic::iced::gradient::Linear;
use cosmic::iced::{
    Background, Border, Color, Gradient, Length, Limits, Radians, Size, Subscription, window,
};
use cosmic::prelude::*;
use cosmic::widget;

use crate::app::{self, Message};
use crate::outputs::Outputs;
use crate::panel::{
    DiskId, GpuRef, Item, Layout, NetRef, PanelSettings, PanelSummary, PopupLayout, Row, Rows, Slot,
};
use crate::sampler::history::{HISTORY_LEN, History};
use crate::sampler::rates::PairRate;
use crate::sampler::stats::{
    DeviceRate, DiskStats, DriveTemp, Extremes, GpuExtremes, GpuHistory, GpuSample, Histories,
    InterfaceHistory, MinMax, Processes, Stats,
};
use crate::sources::drm::RuntimeStatus;
use crate::sources::loadavg::LoadAvg;
use crate::sources::meminfo::MemInfo;
use crate::ui::builder::BuilderChange;
use crate::ui::columns::{self, Columns, Room};
use crate::ui::panel_button::panel_button;
use crate::ui::popup::{self, Page};
use crate::ui::settings::{SettingChange, SettingsModels};
use crate::{redraws, sampler};
use sysmon_proto::{Mount, Process, TopProcesses};

/// The screenshot's default size in logical pixels; wider for two columns.
const SHOT: Size = Size::new(640.0, 900.0);
/// The space either side of the popup in a two-column screenshot.
const SHOT_MARGIN: f32 = 40.0;
/// The width of the output a screenshot stands for: room for two columns.
const SHOT_OUTPUT_WIDTH: f32 = 1920.0;
/// Time for the first frames and the icons to load before the screenshot.
const SHOT_DELAY: Duration = Duration::from_millis(1500);
/// The same, from the live sampler: two samples, so the rates are in.
const LIVE_SHOT_DELAY: Duration = Duration::from_secs(5);

pub fn run() -> cosmic::iced::Result {
    let shot = std::env::var("SYSMON_PREVIEW_SCREENSHOT").ok();
    let settings = if shot.is_some() && panel_only() {
        let horizontal = cosmic::applet::Context::default().is_horizontal();
        let size = if horizontal {
            PANEL_SHOT
        } else {
            Size::new(PANEL_SHOT.height, PANEL_SHOT.width)
        };
        cosmic::app::Settings::default()
            .size(size)
            .size_limits(Limits::new(size, size))
            .resizable(None)
            .theme(cosmic::Theme::dark())
    } else if shot.is_some() {
        let height = shot_height();
        let columns = columns::choose(
            popup_layout(),
            columns::one_column_height(&fixture()),
            Some(shot_room(height)),
            true,
        );
        let width = SHOT.width.max(columns.width() + 2.0 * SHOT_MARGIN);
        let size = Size::new(width, height);
        cosmic::app::Settings::default()
            .size(size)
            // A window that can't be resized floats instead of being tiled to another size.
            .size_limits(Limits::new(size, size))
            .resizable(None)
            .theme(cosmic::Theme::dark())
    } else {
        cosmic::app::Settings::default().size(Size::new(Columns::Two.width() + 40.0, 1000.0))
    };
    cosmic::app::run::<Preview>(settings, shot)
}

/// The size of a screenshot of the panel alone (`SYSMON_PREVIEW_PANEL`), for a horizontal
/// panel; turned on its side for a vertical one.
const PANEL_SHOT: Size = Size::new(560.0, 150.0);

/// Whether only the panel is shown (`SYSMON_PREVIEW_PANEL`).
fn panel_only() -> bool {
    std::env::var_os("SYSMON_PREVIEW_PANEL").is_some()
}

/// The rows from `SYSMON_PREVIEW_ROWS`: `custom` for four items over three, GPU, `NVMe` and
/// disk items among them; else the default rows.
fn rows() -> Rows {
    if std::env::var("SYSMON_PREVIEW_ROWS").as_deref() != Ok("custom") {
        return Rows::default();
    }
    Rows::new(
        vec![
            Item::Cpu,
            Item::GpuUsage(GpuRef::Auto),
            Item::GpuTemp(GpuRef::Pci("0000:05:00.0".into())),
            Item::Download(NetRef::All),
        ],
        vec![
            Item::Ram,
            Item::NvmeTemp(DiskId {
                model: "Samsung SSD 980 PRO 1TB".into(),
                name: "nvme0n1".into(),
            }),
            // Nearly full: it owns the warning, so only it turns red.
            Item::DiskUsage("/mnt/storage".into()),
        ],
    )
}

/// The screenshot window's height, which stands for the output's.
fn shot_height() -> f32 {
    std::env::var("SYSMON_PREVIEW_HEIGHT")
        .ok()
        .and_then(|height| height.parse().ok())
        .unwrap_or(SHOT.height)
}

/// The room in a screenshot `height` high, under the mock panel, as the applet measures it.
fn shot_room(height: f32) -> Room {
    let (_, panel) = cosmic::applet::Context::default().suggested_window_size();
    #[expect(clippy::cast_precision_loss, reason = "a panel's thickness")]
    Room::on_output((SHOT_OUTPUT_WIDTH, height), panel.get() as f32, true)
}

/// The popup's layout from `SYSMON_PREVIEW_COLUMNS`.
fn popup_layout() -> PopupLayout {
    match std::env::var("SYSMON_PREVIEW_COLUMNS").as_deref() {
        Ok("1") => PopupLayout::OneColumn,
        Ok("2") => PopupLayout::TwoColumns,
        _ => PopupLayout::Auto,
    }
}

struct Preview {
    core: cosmic::Core,
    /// Where the screenshot goes, in screenshot mode.
    shot: Option<String>,
    stats: Arc<Stats>,
    settings: PanelSettings,
    models: SettingsModels,
    summary: PanelSummary,
    page: Page,
    /// The outputs' sizes, outside screenshot mode.
    outputs: Outputs<cosmic::cctk::sctk::reexports::client::protocol::wl_output::WlOutput>,
    /// The popup's room: the window's in screenshot mode, else the output's once known.
    room: Option<Room>,
    columns: Columns,
    /// Fed by the real sampler (`SYSMON_PREVIEW_LIVE`).
    live: bool,
    /// The live sampler, once started.
    sampler: Option<sampler::SamplerHandle>,
    /// When the last live sample arrived, for the redraw counts.
    last_sample: Option<Instant>,
}

impl Preview {
    /// Makes `change` on the settings page, here only: the preview stores nothing.
    fn change_setting(&mut self, change: SettingChange) {
        self.settings = self.models.changed(change, &self.settings);
        self.models.select(&self.settings);
        self.summary = PanelSummary::new(&self.stats, &self.settings);
        self.choose_columns();
    }

    /// As the applet chooses them, from the room in the screenshot or on the output.
    fn choose_columns(&mut self) {
        let applet = &self.core.applet;
        let horizontal = applet.is_horizontal();
        if self.shot.is_none() {
            self.room = self.outputs.size(&applet.output_name).map(|output| {
                let (width, height) = applet.suggested_window_size();
                let panel = if horizontal { height } else { width };
                #[expect(clippy::cast_precision_loss, reason = "a panel's thickness")]
                Room::on_output(output, panel.get() as f32, horizontal)
            });
        }
        let height = columns::one_column_height(&self.stats);
        self.columns = columns::choose(self.settings.popup_layout, height, self.room, horizontal);
    }
}

impl cosmic::Application for Preview {
    type Executor = cosmic::executor::Default;
    type Flags = Option<String>;
    type Message = Message;

    const APP_ID: &'static str = "io.github.leonardo_panseri.cosmic-ext-applet-sysmon.Preview";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(mut core: cosmic::Core, shot: Self::Flags) -> (Self, Task<cosmic::Action<Message>>) {
        let live = std::env::var_os("SYSMON_PREVIEW_LIVE").is_some();
        let task = match (&shot, core.main_window_id()) {
            (Some(path), Some(id)) => {
                // Only the view: no header bar, padding or rounded corners.
                core.window.show_headerbar = false;
                core.window.content_container = false;
                core.window.sharp_corners = true;
                let delay = if live { LIVE_SHOT_DELAY } else { SHOT_DELAY };
                screenshot(id, path.clone(), delay)
            }
            _ => Task::none(),
        };
        let stats = Arc::new(fixture());
        let settings = PanelSettings {
            layout: match std::env::var("SYSMON_PREVIEW_LAYOUT").as_deref() {
                Ok("one-line") => Layout::OneLine,
                _ => Layout::Stacked,
            },
            rows: rows(),
            popup_layout: popup_layout(),
            ..PanelSettings::default()
        };
        let summary = PanelSummary::new(&stats, &settings);
        let room = shot.as_ref().map(|_| shot_room(shot_height()));
        let mut preview = Self {
            core,
            shot,
            stats,
            models: SettingsModels::new(&settings),
            settings,
            summary,
            page: if std::env::var_os("SYSMON_PREVIEW_BUILDER").is_some() {
                Page::Builder
            } else if std::env::var_os("SYSMON_PREVIEW_SETTINGS").is_some() {
                Page::Settings
            } else {
                Page::Stats
            },
            outputs: Outputs::default(),
            room,
            columns: Columns::One,
            live,
            sampler: None,
            last_sample: None,
        };
        // The builder with a chip selected, or its picker open.
        let start = match std::env::var("SYSMON_PREVIEW_BUILDER").as_deref() {
            Ok("selected") => Some(BuilderChange::Select(Slot {
                row: Row::Top,
                index: 2,
            })),
            Ok("picker") => Some(BuilderChange::OpenPicker(Row::Bottom)),
            _ => None,
        };
        if let Some(change) = start {
            preview.change_setting(SettingChange::Builder(change));
        }
        preview.choose_columns();
        (preview, task)
    }

    fn subscription(&self) -> Subscription<Message> {
        let outputs = if self.shot.is_none() {
            app::output_events()
        } else {
            Subscription::none()
        };
        if self.live {
            Subscription::batch([Subscription::run(app::sampler_messages), outputs])
        } else {
            outputs
        }
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        redraws::count(redraws::Event::Update);
        match message {
            Message::SamplerStarted(handle) => {
                handle.send(sampler::Control::Settings(self.settings.clone()));
                self.sampler = Some(handle);
            }
            Message::Sampled(update) => {
                redraws::log_on_sample(&mut self.last_sample);
                self.summary = update.panel;
                self.stats = update.stats;
            }
            Message::Output(event, output) => {
                app::track_output(&mut self.outputs, event, output);
                self.choose_columns();
            }
            Message::ShowPage(page) => {
                self.change_setting(SettingChange::CommitAll);
                if page != self.page {
                    self.models.reset_builder();
                }
                self.page = page;
            }
            Message::OpenProcessMonitor => eprintln!(
                "preview: would open the process monitor {:?}",
                self.settings.process_monitor
            ),
            Message::Setting(change) => self.change_setting(change),
            _ => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        redraws::count(redraws::Event::ViewWindow);
        let popup = card(popup::view(
            Some(&self.stats),
            &self.settings,
            &self.models,
            self.page,
            self.columns,
            columns::page_max_height(self.room),
        ));
        let button = panel_button(&self.core.applet, &self.settings, Some(&self.summary))
            .on_press_down(Message::TogglePopup);
        let panel = widget::container(button)
            .padding([2, 12])
            .class(cosmic::theme::Container::Background);
        if self.shot.is_none() {
            return widget::scrollable(
                widget::Column::new()
                    .spacing(12)
                    .padding(20)
                    .push(panel)
                    .push(popup),
            )
            .into();
        }
        if panel_only() {
            return panel_strip(panel, self.core.applet.is_horizontal());
        }
        // A top panel with the button near its right end, and the popup under the button.
        let panel = widget::container(
            widget::Row::new()
                .push(widget::Space::new().width(Length::Fill))
                .push(panel),
        )
        .padding([0, 40, 0, 0])
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            widget::container::Style {
                text_color: Some(cosmic.background(false).on.into()),
                icon_color: Some(cosmic.background(false).on.into()),
                background: Some(Color::from(cosmic.background(false).base).into()),
                ..widget::container::Style::default()
            }
        }));
        // Near the panel's end, as the compositor slides a wide popup to stay on screen.
        let popup = widget::Row::new()
            .push(widget::Space::new().width(Length::Fill))
            .push(popup)
            .padding([0, 24, 0, 0]);
        widget::container(widget::Column::new().spacing(8).push(panel).push(popup))
            .width(Length::Fill)
            .height(Length::Fill)
            .class(cosmic::theme::Container::custom(|_| {
                widget::container::Style {
                    background: Some(backdrop()),
                    ..widget::container::Style::default()
                }
            }))
            .into()
    }
}

/// `panel` alone over the backdrop: a top panel holding it near its middle, or a left panel
/// holding it near its top.
fn panel_strip<'a>(
    panel: widget::Container<'a, Message, cosmic::Theme>,
    horizontal: bool,
) -> Element<'a, Message> {
    let style = |theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        widget::container::Style {
            text_color: Some(cosmic.background(false).on.into()),
            icon_color: Some(cosmic.background(false).on.into()),
            background: Some(Color::from(cosmic.background(false).base).into()),
            ..widget::container::Style::default()
        }
    };
    let strip: Element<'a, Message> = if horizontal {
        widget::container(panel)
            .center_x(Length::Fill)
            .class(cosmic::theme::Container::custom(style))
            .into()
    } else {
        widget::container(panel.padding([12, 2]))
            .padding([24, 0])
            .height(Length::Fill)
            .class(cosmic::theme::Container::custom(style))
            .into()
    };
    widget::container(strip)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::custom(|_| {
            widget::container::Style {
                background: Some(backdrop()),
                ..widget::container::Style::default()
            }
        }))
        .into()
}

/// `content`, as wide as it lays out, in the look of the panel's popup container.
fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content)
        .class(cosmic::theme::Container::custom(|theme| {
            let cosmic = theme.cosmic();
            let background = cosmic.background(theme.transparent);
            widget::container::Style {
                text_color: Some(background.on.into()),
                icon_color: Some(background.on.into()),
                background: Some(Color::from(background.base).into()),
                border: Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 1.0,
                    color: background.divider.into(),
                },
                ..widget::container::Style::default()
            }
        }))
        .into()
}

/// A dark blue-green gradient, standing in for a wallpaper.
fn backdrop() -> Background {
    Gradient::Linear(
        Linear::new(Radians(2.4))
            .add_stop(0.0, Color::from_rgb8(0x0b, 0x3d, 0x3a))
            .add_stop(1.0, Color::from_rgb8(0x0a, 0x16, 0x24)),
    )
    .into()
}

/// A task that completes after `duration`, on a timer thread: the applet doesn't depend on
/// tokio itself.
fn sleep<T: Send + 'static>(duration: Duration) -> Task<T> {
    let (done, delay) = cosmic::iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = done.send(());
    });
    Task::future(delay).discard()
}

/// After `delay`, saves the window's pixels to `path` and exits.
fn screenshot(id: window::Id, path: String, delay: Duration) -> Task<cosmic::Action<Message>> {
    sleep(delay).chain(window::screenshot(id).then(move |shot| {
        let mut file = b"RGBA".to_vec();
        file.extend_from_slice(&shot.size.width.to_le_bytes());
        file.extend_from_slice(&shot.size.height.to_le_bytes());
        file.extend_from_slice(&shot.rgba);
        let code = match std::fs::write(&path, file) {
            Ok(()) => 0,
            Err(why) => {
                eprintln!("can't write {path}: {why}");
                1
            }
        };
        // Not `iced::exit`: tearing the window down sometimes crashes, after the file is
        // written but with an exit status the script would take for a failure.
        std::process::exit(code);
        #[allow(unreachable_code)]
        Task::none()
    }))
}

const KIB: f64 = 1024.0;
const MIB: f64 = 1024.0 * KIB;
const GIB: u64 = 1 << 30;

/// A full history: a smooth wave around `base`, `swing` either way, plus a little ripple,
/// clamped at zero.
fn wave(base: f32, swing: f32, phase: f32) -> History {
    let mut history = History::default();
    for i in 0..HISTORY_LEN {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        let t = i as f32;
        let value =
            base + swing * (t / 7.0 + phase).sin() + swing * 0.35 * (t / 1.7 + phase * 3.0).sin();
        history.push(value.max(0.0));
    }
    history
}

/// A history of zeros: a suspended GPU.
fn idle() -> History {
    let mut history = History::default();
    for _ in 0..HISTORY_LEN {
        history.push(0.0);
    }
    history
}

#[expect(clippy::cast_possible_truncation, reason = "graph values")]
fn wave_bytes(base: f64, swing: f64, phase: f32) -> History {
    wave(base as f32, swing as f32, phase)
}

fn rate(first: f64, second: f64) -> PairRate {
    PairRate { first, second }
}

fn process(pid: u32, name: &str, cpu: f32, rss: u64) -> Process {
    Process {
        pid,
        name: name.into(),
        cpu,
        rss,
    }
}

fn mount(mount_point: &str, device: &str, total: u64, used: u64) -> Mount {
    Mount {
        mount_point: mount_point.into(),
        device: device.into(),
        fs_type: if mount_point == "/boot" {
            "vfat"
        } else {
            "ext4"
        }
        .into(),
        total,
        used,
        // As ext4 reserves 5 % for root.
        available: (total - used) - total / 20,
    }
}

/// Realistic stats for a laptop with a suspended NVIDIA dGPU and an AMD iGPU.
#[must_use]
pub fn fixture() -> Stats {
    let cores = [
        38.0, 52.0, 12.0, 91.0, 27.0, 44.0, 18.0, 63.0, 35.0, 22.0, 87.0, 41.0, 15.0, 58.0, 30.0,
        47.0,
    ];
    let net = vec![
        DeviceRate {
            name: "wlp4s0".into(),
            rate: Some(rate(1.1 * MIB, 210.0 * KIB)),
        },
        DeviceRate {
            name: "enp5s0".into(),
            rate: Some(rate(340.0 * KIB, 820.0 * KIB)),
        },
    ];
    let history = Histories {
        cpu: wave(38.0, 14.0, 0.0),
        memory: wave(39.0, 4.0, 0.7),
        swap: wave(8.0, 3.0, 2.4),
        gpus: vec![
            GpuHistory {
                card: "card0".into(),
                busy: idle(),
                vram: idle(),
            },
            GpuHistory {
                card: "card1".into(),
                busy: wave(30.0, 18.0, 1.3),
                vram: wave(42.0, 6.0, 0.2),
            },
        ],
        net_down: wave_bytes(1.4 * MIB, 0.9 * MIB, 0.4),
        net_up: wave_bytes(0.9 * MIB, 0.6 * MIB, 2.1),
        interfaces: vec![
            InterfaceHistory {
                name: "wlp4s0".into(),
                down: wave_bytes(1.1 * MIB, 0.8 * MIB, 0.4),
                up: wave_bytes(200.0 * KIB, 150.0 * KIB, 2.1),
            },
            InterfaceHistory {
                name: "enp5s0".into(),
                down: wave_bytes(300.0 * KIB, 250.0 * KIB, 1.0),
                up: wave_bytes(700.0 * KIB, 500.0 * KIB, 3.0),
            },
        ],
    };
    Stats {
        cpu: Some(41.0),
        cores: cores.into_iter().map(Some).collect(),
        mem: Some(MemInfo {
            mem_total: 30 * GIB,
            mem_available: 30 * GIB - 11_811_160_064,
            swap_total: 8 * GIB,
            swap_free: 8 * GIB - 644_245_094,
        }),
        load: Some(LoadAvg {
            one: 1.84,
            five: 1.32,
            fifteen: 1.31,
        }),
        uptime: Some(Duration::from_mins((3 * 24 + 5) * 60 + 17)),
        net_total: Some(rate(1.1 * MIB + 340.0 * KIB, 210.0 * KIB + 820.0 * KIB)),
        net,
        disk_total: Some(rate(3.0 * MIB, 780.0 * KIB)),
        cpu_temp: Some(54.0),
        drives: drives().to_vec(),
        gpus: gpus(),
        mounts: mounts(),
        disks: disks(),
        other_mounts: Vec::new(),
        processes: Processes::Top(processes()),
        history,
        extremes: Extremes {
            cpu: Some(range(3.0, 97.0)),
            cpu_temp: Some(range(42.0, 88.0)),
            memory: Some(range(gib(9.3), gib(17.4))),
            swap: Some(range(0.0, gib(1.2))),
        },
    }
}

fn range(min: f32, max: f32) -> MinMax {
    MinMax { min, max }
}

/// `value` GiB, as a size [`MinMax`] holds.
#[expect(clippy::cast_precision_loss, reason = "preview sizes")]
fn gib(value: f32) -> f32 {
    value * GIB as f32
}

fn drives() -> [DriveTemp; 2] {
    [
        DriveTemp {
            name: "Samsung SSD 980 PRO 1TB".into(),
            celsius: 41.0,
            limit: Some(82.0),
            extremes: Some(range(36.0, 58.0)),
        },
        DriveTemp {
            name: "WD_BLACK SN850X 2TB".into(),
            celsius: 38.0,
            limit: Some(84.0),
            extremes: Some(range(33.0, 51.0)),
        },
    ]
}

/// The two drives of [`drives`], with the filesystems of [`mounts`] on them.
fn disks() -> Vec<DiskStats> {
    let [samsung, wd] = drives();
    let mounts = mounts();
    // The LUKS mapping is on `nvme0n1p3`.
    let on = |sources: &[&str]| {
        mounts
            .iter()
            .filter(|m| sources.iter().any(|s| m.device.starts_with(s)))
            .cloned()
            .collect()
    };
    vec![
        DiskStats {
            name: "nvme0n1".into(),
            model: samsung.name.clone(),
            rate: Some(rate(2.6 * MIB, 512.0 * KIB)),
            temp: Some(samsung),
            mounts: on(&["/dev/nvme0n1", "/dev/mapper/luks-home"]),
        },
        DiskStats {
            name: "nvme1n1".into(),
            model: wd.name.clone(),
            rate: Some(rate(400.0 * KIB, 268.0 * KIB)),
            temp: Some(wd),
            mounts: on(&["/dev/nvme1n1"]),
        },
    ]
}

/// A suspended NVIDIA dGPU, named from procfs, with its VRAM size from the cache (D4), and an
/// AMD iGPU named from `pci.ids`.
fn gpus() -> Vec<GpuSample> {
    vec![
        GpuSample {
            card: "card0".into(),
            pci: Some("0000:01:00.0".into()),
            nvidia: true,
            name: Some("NVIDIA GeForce RTX 4060 Laptop GPU".into()),
            uuid: Some("GPU-6e9f6c15-581b-4f11-c568-db1698fe7502".into()),
            status: RuntimeStatus::Suspended,
            vram_used: Some(0),
            vram_total: Some(8188 << 20),
            // From the last time it was awake.
            extremes: GpuExtremes {
                busy: Some(range(0.0, 100.0)),
                celsius: Some(range(45.0, 79.0)),
                vram: Some(range(gib(0.3), gib(4.9))),
            },
            ..GpuSample::default()
        },
        GpuSample {
            card: "card1".into(),
            pci: Some("0000:05:00.0".into()),
            name: Some("AMD Radeon 680M".into()),
            status: RuntimeStatus::Active,
            busy: Some(33),
            vram_used: Some(644_245_094),
            vram_total: Some(2 * GIB),
            celsius: Some(51.0),
            extremes: GpuExtremes {
                busy: Some(range(0.0, 96.0)),
                celsius: Some(range(38.0, 71.0)),
                vram: Some(range(gib(0.25), gib(0.9))),
            },
            ..GpuSample::default()
        },
    ]
}

/// Four filesystems, `/home` on LUKS and the last one nearly full.
fn mounts() -> Vec<Mount> {
    vec![
        mount("/", "/dev/nvme0n1p2", 220 * GIB, 98 * GIB),
        mount("/boot", "/dev/nvme0n1p1", GIB, 188 << 20),
        mount("/home", "/dev/mapper/luks-home", 700 * GIB, 402 * GIB),
        mount("/mnt/storage", "/dev/nvme1n1p1", 1_830 * GIB, 1_690 * GIB),
    ]
}

fn processes() -> TopProcesses {
    TopProcesses {
        by_cpu: vec![
            process(4121, "firefox", 38.2, 2_254_857_830),
            process(1630, "cosmic-comp", 12.4, 412_000_000),
            process(9012, "cargo", 9.8, 610_000_000),
            process(2210, "pipewire", 3.1, 48_000_000),
            process(7781, "Isolated Web Content", 2.6, 920_000_000),
        ],
        by_memory: vec![
            process(4121, "firefox", 38.2, 2_254_857_830),
            process(5520, "rust-analyzer", 0.4, 1_610_612_736),
            process(7781, "Isolated Web Content", 2.6, 920_000_000),
            process(9012, "cargo", 9.8, 610_000_000),
            process(1630, "cosmic-comp", 12.4, 412_000_000),
        ],
    }
}
