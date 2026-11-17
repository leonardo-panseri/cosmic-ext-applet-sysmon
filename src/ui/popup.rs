// SPDX-License-Identifier: MPL-2.0

//! An interim popup: the open tier's values as plain lines, so the sampler can be checked live.
//! The stats page of §2.2 replaces it in P3, with localized labels and graphs.

use cosmic::prelude::*;
use cosmic::widget;

use crate::app::Message;
use crate::fl;
use crate::panel::format;
use crate::sampler::rates::PairRate;
use crate::sampler::stats::Stats;

#[expect(
    clippy::too_many_lines,
    reason = "a flat list of lines, replaced in P3"
)]
/// The lines of the interim popup for `stats`, or a placeholder before the first sample.
pub fn interim_stats<'a>(stats: Option<&Stats>) -> Element<'a, Message> {
    let Some(stats) = stats else {
        return widget::text::body(fl!("placeholder")).into();
    };
    let dash = || fl!("no-value");
    let pct = |v: Option<f32>| v.map_or_else(dash, format::percent);
    let deg = |v: Option<f32>| v.map_or_else(dash, format::celsius);
    let rates = |r: Option<PairRate>| {
        r.map_or_else(dash, |r| {
            format!("{} / {}", format::rate(r.first), format::rate(r.second))
        })
    };
    let gib = |bytes: u64| {
        #[expect(clippy::cast_precision_loss, reason = "display only")]
        let gib = bytes as f64 / f64::from(1 << 30);
        format!("{gib:.1}G")
    };
    let mut lines = vec![
        format!(
            "CPU {} · {} threads · {}",
            pct(stats.cpu),
            stats.cores.len(),
            deg(stats.cpu_temp)
        ),
        format!(
            "Cores {}",
            stats
                .cores
                .iter()
                .map(|c| c.map_or_else(dash, |c| format!("{c:.0}")))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        stats.load.map_or_else(dash, |l| {
            format!("Load {:.2} {:.2} {:.2}", l.one, l.five, l.fifteen)
        }),
        stats.uptime.map_or_else(dash, |u| {
            let mins = u.as_secs() / 60;
            format!("Up {}h {:02}m", mins / 60, mins % 60)
        }),
        stats.mem.map_or_else(dash, |m| {
            format!(
                "RAM {} / {} · swap {} / {}",
                gib(m.mem_used()),
                gib(m.mem_total),
                gib(m.swap_used()),
                gib(m.swap_total)
            )
        }),
        format!("Net ↓/↑ {}", rates(stats.net_total)),
    ];
    lines.extend(
        stats
            .net
            .iter()
            .map(|i| format!("  {} {}", i.name, rates(i.rate))),
    );
    lines.push(format!("Disk r/w {}", rates(stats.disk_total)));
    lines.extend(
        stats
            .disks
            .iter()
            .map(|d| format!("  {} {}", d.name, rates(d.rate))),
    );
    lines.extend(stats.mounts.iter().map(|m| {
        format!(
            "{} {} {} · {} / {} · {}",
            m.mount_point,
            m.device,
            m.fs_type,
            gib(m.used),
            gib(m.total),
            pct(m.used_percent())
        )
    }));
    lines.extend(stats.drives.iter().map(|d| {
        format!(
            "{} {} (limit {})",
            d.name,
            format::celsius(d.celsius),
            deg(d.limit)
        )
    }));
    lines.extend(stats.gpus.iter().map(|g| {
        let vram = g
            .vram_used
            .zip(g.vram_total)
            .map_or_else(dash, |(used, total)| {
                format!("{} / {}", gib(used), gib(total))
            });
        format!(
            "{} {} {:?} · {} · {} · VRAM {vram}",
            g.card,
            g.name.as_deref().unwrap_or_default(),
            g.status,
            pct(g.busy.map(f32::from)),
            deg(g.celsius)
        )
    }));
    if let Some(top) = &stats.processes {
        lines.push("Top CPU".into());
        lines.extend(
            top.by_cpu
                .iter()
                .map(|p| format!("  {} {} {:.1}%", p.pid, p.name, p.cpu)),
        );
        lines.push("Top memory".into());
        lines.extend(
            top.by_memory
                .iter()
                .map(|p| format!("  {} {} {}", p.pid, p.name, gib(p.rss))),
        );
    }
    let history = &stats.history;
    lines.push(format!(
        "History: CPU {}, net {}, GPUs {} (latest CPU {})",
        history.cpu.iter().len(),
        history.net_down.iter().len(),
        history.gpus.len(),
        pct(history.cpu.latest())
    ));
    widget::Column::with_children(lines.into_iter().map(|l| widget::text::body(l).into()))
        .spacing(4)
        .into()
}
