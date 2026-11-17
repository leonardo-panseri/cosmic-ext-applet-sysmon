// SPDX-License-Identifier: MPL-2.0

//! Debug builds only: `SYSMON_PREVIEW=1 cargo run` shows the panel button and the popup in an
//! ordinary window, fed with fixed, made-up [`Stats`] ([`fixture`]): two GPUs (a suspended
//! NVIDIA card and an AMD iGPU), two network interfaces, three filesystems, two `NVMe` drives and
//! full histories. Nothing is read from the system, so no GPU is woken. The **Settings** button
//! switches pages; settings apply to the preview only; **Process monitor** only prints.
//!
//! `SYSMON_PREVIEW_SCREENSHOT=<file>` instead draws a mock top panel over a backdrop, in COSMIC's
//! default dark theme, saves the window's pixels to `<file>` and exits. `SYSMON_PREVIEW_HEIGHT`
//! sets the window's height in that mode, and `SYSMON_PREVIEW_FULL=1` shows the whole stats page
//! at once, cut in two halves side by side, instead of scrolling it. `SYSMON_PREVIEW_SETTINGS=1`
//! starts on the settings page. The file is `RGBA`, the width and height as
//! little-endian `u32`s, then the pixels; `tools/screenshots.py` turns it into a PNG.

use std::time::Duration;

use cosmic::iced::gradient::Linear;
use cosmic::iced::{Background, Border, Color, Gradient, Length, Limits, Radians, Size, window};
use cosmic::prelude::*;
use cosmic::widget;

use crate::app::Message;
use crate::panel::{PanelSettings, PanelSummary};
use crate::sampler::history::{HISTORY_LEN, History};
use crate::sampler::rates::PairRate;
use crate::sampler::stats::{DeviceRate, DriveTemp, GpuSample, Histories, InterfaceHistory, Stats};
use crate::sources::drm::RuntimeStatus;
use crate::sources::loadavg::LoadAvg;
use crate::sources::meminfo::MemInfo;
use crate::ui::panel_button::panel_button;
use crate::ui::popup::{self, Page};
use crate::ui::settings::SettingsModels;
use sysmon_proto::{Mount, Process, TopProcesses};

/// As wide as the panel's popup at most.
const WIDTH: f32 = 372.0;
/// The screenshot's default size in logical pixels.
const SHOT: Size = Size::new(640.0, 900.0);
/// Time for the first frames and the icons to load before the screenshot.
const SHOT_DELAY: Duration = Duration::from_millis(1500);

pub fn run() -> cosmic::iced::Result {
    let shot = std::env::var("SYSMON_PREVIEW_SCREENSHOT").ok();
    let settings = if shot.is_some() {
        let height = std::env::var("SYSMON_PREVIEW_HEIGHT")
            .ok()
            .and_then(|height| height.parse().ok())
            .unwrap_or(SHOT.height);
        let width = if std::env::var_os("SYSMON_PREVIEW_FULL").is_some() {
            2.0 * WIDTH + 80.0
        } else {
            SHOT.width
        };
        let size = Size::new(width, height);
        cosmic::app::Settings::default()
            .size(size)
            // A window that can't be resized floats instead of being tiled to another size.
            .size_limits(Limits::new(size, size))
            .resizable(None)
            .theme(cosmic::Theme::dark())
    } else {
        cosmic::app::Settings::default().size(Size::new(WIDTH + 40.0, 1000.0))
    };
    cosmic::app::run::<Preview>(settings, shot)
}

struct Preview {
    core: cosmic::Core,
    /// Where the screenshot goes, in screenshot mode.
    shot: Option<String>,
    stats: Stats,
    settings: PanelSettings,
    models: SettingsModels,
    summary: PanelSummary,
    page: Page,
    /// The stats page grows instead of scrolling.
    full: bool,
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
        let task = match (&shot, core.main_window_id()) {
            (Some(path), Some(id)) => {
                // Only the view: no header bar, padding or rounded corners.
                core.window.show_headerbar = false;
                core.window.content_container = false;
                core.window.sharp_corners = true;
                screenshot(id, path.clone())
            }
            _ => Task::none(),
        };
        let stats = fixture();
        let settings = PanelSettings::default();
        let summary = PanelSummary::new(
            &stats,
            &settings,
            settings.layout_for(core.applet.is_horizontal()),
        );
        let preview = Self {
            core,
            shot,
            stats,
            models: SettingsModels::new(&settings),
            settings,
            summary,
            page: if std::env::var_os("SYSMON_PREVIEW_SETTINGS").is_some() {
                Page::Settings
            } else {
                Page::Stats
            },
            full: std::env::var_os("SYSMON_PREVIEW_FULL").is_some(),
        };
        (preview, task)
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::ShowPage(page) => self.page = page,
            Message::OpenProcessMonitor => eprintln!("preview: would open the process monitor"),
            // Applied here only: the preview stores nothing.
            Message::Setting(change) => {
                self.settings = self.models.changed(change, &self.settings);
                self.models.select(&self.settings);
                let layout = self.settings.layout_for(self.core.applet.is_horizontal());
                self.summary = PanelSummary::new(&self.stats, &self.settings, layout);
            }
            _ => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let popup: Element<'_, Message> = if self.full && self.page == Page::Stats {
            let [first, second] = popup::stats_page_halves(&self.stats, &self.settings, 3);
            widget::Row::new()
                .spacing(16)
                .align_y(cosmic::iced::Alignment::Start)
                .push(card(first))
                .push(card(second))
                .into()
        } else {
            card(popup::view(
                Some(&self.stats),
                &self.settings,
                &self.models,
                self.page,
                popup::PAGE_MAX_HEIGHT,
            ))
        };
        let button = panel_button(&self.core.applet, &self.settings, Some(&self.summary));
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

/// `content` in the look of the panel's popup container.
fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content)
        .width(Length::Fixed(WIDTH))
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

/// After [`SHOT_DELAY`], saves the window's pixels to `path` and exits.
fn screenshot(id: window::Id, path: String) -> Task<cosmic::Action<Message>> {
    sleep(SHOT_DELAY).chain(window::screenshot(id).then(move |shot| {
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
        fs_type: "ext4".into(),
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
        gpus: vec![
            ("card0".into(), idle()),
            ("card1".into(), wave(30.0, 18.0, 1.3)),
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
            swap_free: 8 * GIB,
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
        drives: vec![
            DriveTemp {
                name: "Samsung SSD 980 PRO 1TB".into(),
                celsius: 41.0,
                limit: Some(82.0),
            },
            DriveTemp {
                name: "WD_BLACK SN850X 2TB".into(),
                celsius: 38.0,
                limit: Some(84.0),
            },
        ],
        gpus: gpus(),
        mounts: mounts(),
        processes: Some(processes()),
        history,
    }
}

/// A suspended NVIDIA dGPU, named from procfs, with its VRAM size from the cache (D4), and an
/// AMD iGPU named from `pci.ids`.
fn gpus() -> Vec<GpuSample> {
    vec![
        GpuSample {
            card: "card0".into(),
            nvidia: true,
            name: Some("NVIDIA GeForce RTX 4060 Laptop GPU".into()),
            uuid: Some("GPU-6e9f6c15-581b-4f11-c568-db1698fe7502".into()),
            status: RuntimeStatus::Suspended,
            vram_used: Some(0),
            vram_total: Some(8188 << 20),
            ..GpuSample::default()
        },
        GpuSample {
            card: "card1".into(),
            name: Some("AMD Radeon 680M".into()),
            status: RuntimeStatus::Active,
            busy: Some(33),
            vram_used: Some(644_245_094),
            vram_total: Some(2 * GIB),
            celsius: Some(51.0),
            ..GpuSample::default()
        },
    ]
}

/// Three filesystems, the last one nearly full.
fn mounts() -> Vec<Mount> {
    vec![
        mount("/", "/dev/nvme0n1p2", 928 * GIB, 212 * GIB),
        mount("/home", "/dev/nvme0n1p3", 1_800 * GIB, 1_402 * GIB),
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
