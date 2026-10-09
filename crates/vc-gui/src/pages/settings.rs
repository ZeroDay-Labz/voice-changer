//! Settings: device, window behaviour, hotkey, theme, compute, logs.

use super::{dim, kv, section, section_accent, section_with};
use crate::help;
use crate::host::{Host, HostSetting};
use crate::theme::{self, Mode};
use crate::widgets::icons;
use crate::{Element, Message, Model, tip};
use iced_core::{Alignment, Length};
use iced_widget::{button, column, container, pick_list, row, scrollable, text, toggler};

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let caps = host.capabilities();
    let mut col = column![].spacing(16);
    if caps.standalone {
        col = col.push(audio(model, host, mode));
        col = col.push(window(host, mode));
    }
    col = col.push(appearance(host, mode));
    if caps.standalone {
        col = col.push(logs(host, mode));
    }
    col = col.push(about(host, mode));
    col.into()
}

fn labelled<'a>(label: &'a str, mode: Mode, content: impl Into<Element<'a>>) -> Element<'a> {
    let t = theme::tokens(mode);
    row![
        container(text(label).size(14).color(t.text_dim)).width(Length::Fixed(150.0)),
        content.into()
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn audio<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let mut col = column![].spacing(10);
    if host.capabilities().microphone {
        col = col.push(labelled(
            "Microphone",
            mode,
            super::topbar::mic_picker(model, host, 320.0, true),
        ));
    }
    if let Some(on) = host.monitor() {
        col = col.push(labelled(
            "Hear myself",
            mode,
            toggler(on)
                .on_toggle(|_| Message::ToggleMonitor)
                .style(theme::toggler_style),
        ));
        col = col.push(dim("Plays the processed voice to your default output so you can hear what others hear. Use headphones.", 12, mode));
    }
    if let Some(stats) = host.stats() {
        col = col.push(kv(
            "Block",
            format!(
                "{} µs ({:.1} ms latency)",
                stats.quantum_us,
                host.latency_ms()
            ),
            mode,
        ));
        col = col.push(kv(
            "Processing peak",
            format!("{} µs", stats.process_max_us),
            mode,
        ));
        col = col.push(kv("Dropouts", stats.underruns.to_string(), mode));
        if stats.underruns > 0 {
            col = col.push(dim(
                "Dropouts: start with a larger block, e.g. `voice-changer --quantum 512`.",
                12,
                mode,
            ));
        }
    }
    let mut out = column![section_accent("Audio", theme::SKY, mode, col)].spacing(16);
    if host.capabilities().standalone {
        out = out.push(super::mixer::send_to(model, mode));
    }
    out.into()
}

fn window<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let s = host.settings();
    let mut col = column![
        labelled(
            "Close to tray",
            mode,
            tip(
                toggler(s.close_to_tray)
                    .on_toggle(|b| Message::Setting(HostSetting::CloseToTray(b)))
                    .style(theme::toggler_style),
                help::CLOSE_TO_TRAY
            )
        ),
        dim(
            "When on, the window's X hides Voice Changer in the tray instead of quitting.",
            12,
            mode
        ),
        labelled(
            "Start minimized",
            mode,
            tip(
                toggler(s.start_minimized)
                    .on_toggle(|b| Message::Setting(HostSetting::StartMinimized(b)))
                    .style(theme::toggler_style),
                help::START_MINIMIZED
            )
        ),
    ]
    .spacing(10);
    if let Some(auto) = s.autostart {
        col = col.push(labelled(
            "Start with the desktop",
            mode,
            tip(
                toggler(auto)
                    .on_toggle(|b| Message::Setting(HostSetting::Autostart(b)))
                    .style(theme::toggler_style),
                help::AUTOSTART,
            ),
        ));
    }
    if let Some(hotkey) = host.hotkey() {
        let t = theme::tokens(mode);
        let current: Element<'a> = match hotkey {
            Some(k) if !k.is_empty() => {
                container(text(k).size(13).font(iced_core::Font::MONOSPACE))
                    .padding([4, 8])
                    .style(theme::raised)
                    .into()
            }
            Some(_) => dim("not set", 13, mode).into(),
            None => dim(
                "portal unavailable — bind `voice-changer toggle` to a key in System Settings",
                13,
                mode,
            )
            .into(),
        };
        col = col.push(labelled(
            "On/off hotkey",
            mode,
            row![
                current,
                tip(
                    button(text("Change…").size(13).color(t.text))
                        .style(theme::button_soft)
                        .padding([6, 10])
                        .on_press(Message::ConfigureHotkey),
                    help::HOTKEY,
                )
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ));
    }
    section("Window & shortcuts", mode, col)
}

fn appearance<'a>(_host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let options: Vec<String> = vec!["Dark".into(), "Light".into()];
    let selected = match mode {
        Mode::Dark => "Dark",
        Mode::Light => "Light",
    }
    .to_string();
    let picker = pick_list(options, Some(selected), |label: String| {
        Message::Setting(HostSetting::Theme(if label == "Light" {
            Mode::Light
        } else {
            Mode::Dark
        }))
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(160.0));
    section(
        "Appearance",
        mode,
        labelled("Theme", mode, tip(picker, help::THEME)),
    )
}

fn logs<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let lines = host.log_lines();
    let body: Element<'a> = if lines.is_empty() {
        dim("Nothing logged yet.", 12, mode).into()
    } else {
        let mut col = column![].spacing(1);
        for l in lines
            .into_iter()
            .rev()
            .take(200)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            col = col.push(
                text(l)
                    .size(11)
                    .font(iced_core::Font::MONOSPACE)
                    .color(t.text_dim)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
        container(
            scrollable(col)
                .height(Length::Fixed(200.0))
                .width(Length::Fill),
        )
        .padding(10)
        .width(Length::Fill)
        .style(theme::raised)
        .into()
    };
    let open = tip(
        button(
            row![
                icons::folder(14.0, t.text),
                text("Open data folder").size(13)
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .style(theme::button_soft)
        .padding([6, 10])
        .on_press(Message::OpenDataFolder),
        help::OPEN_DATA,
    );
    section_with("Log", open, mode, body)
}

fn about<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let caps = host.capabilities();
    let mut col = column![
        kv("Version", env!("CARGO_PKG_VERSION"), mode),
        kv("Running as", if caps.standalone { "standalone (PipeWire virtual microphone)" } else { "plugin inside a host" }, mode),
        dim("Realtime voice changer with natural pitch shifting, effects and local AI voices. MIT licensed.", 12, mode),
    ]
    .spacing(8);
    if caps.standalone {
        col = col.push(tip(
            button(text("Quit Voice Changer").size(13).color(t.text))
                .style(theme::button_danger)
                .padding([6, 10])
                .on_press(Message::Quit),
            help::QUIT,
        ));
    }
    section_accent("About", theme::ACCENT, mode, col)
}
