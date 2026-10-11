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
    #[cfg(feature = "ai")]
    {
        col = col.push(ai(model, mode));
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
    if let Some(a) = &model.cache.audio {
        let (line, color) = match a.state {
            crate::AudioState::Running => (
                if a.virtual_mic {
                    "Running. “Voice Changer Mic” is available to every app.".to_string()
                } else {
                    "Running, but the virtual microphone is not connected yet.".to_string()
                },
                theme::ACCENT,
            ),
            crate::AudioState::Starting => ("Starting…".to_string(), theme::tokens(mode).text_dim),
            crate::AudioState::Reconnecting => {
                ("Reconnecting to the audio system…".to_string(), theme::WARN)
            }
            crate::AudioState::Failed => (a.message.clone(), theme::DANGER),
        };
        col = col.push(
            text(line)
                .size(13)
                .color(color)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        );
        if a.state == crate::AudioState::Running && !a.message.is_empty() {
            col = col.push(
                text(a.message.clone())
                    .size(12)
                    .color(theme::WARN)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
        if let (Some(active), Some(chosen)) = (&a.active_mic, &a.chosen_mic)
            && active != chosen
        {
            col = col.push(
                text(format!("Using {active} while {chosen} is unplugged."))
                    .size(12)
                    .color(theme::WARN),
            );
        }
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

/// AI engine: which library runs it, how many threads it gets, and what
/// the machine has.
#[cfg(feature = "ai")]
fn ai<'a>(model: &'a Model, mode: Mode) -> Element<'a> {
    let mut col = column![].spacing(10);
    if let Some(hw) = &model.cache.hardware {
        col = col.push(kv("This computer", hw.line.clone(), mode));
        let max = hw.max_threads.max(1);
        let auto_label = format!("Auto ({})", hw.auto_threads);
        let mut options: Vec<String> = vec![auto_label.clone()];
        options.extend((1..=max).map(|n| n.to_string()));
        let current = model.cache.settings.ai_threads.min(max);
        let selected = if current == 0 {
            auto_label.clone()
        } else {
            current.to_string()
        };
        let picker = pick_list(options, Some(selected), move |label: String| {
            let n = if label.starts_with("Auto") {
                0
            } else {
                label.parse::<u32>().unwrap_or(0)
            };
            Message::Setting(HostSetting::AiThreads(n))
        })
        .style(theme::pick_list_style)
        .text_size(13)
        .width(Length::Fixed(160.0));
        let mut line = row![tip(picker, help::AI_THREADS)]
            .spacing(10)
            .align_y(Alignment::Center);
        if hw.in_use > 0 {
            line = line.push(dim(
                format!(
                    "the loaded voice runs with {} thread{}",
                    hw.in_use,
                    if hw.in_use == 1 { "" } else { "s" }
                ),
                12,
                mode,
            ));
        }
        col = col.push(labelled("AI threads", mode, line));
        col = col.push(dim(
            "Auto keeps one core free for audio and the window. Pick a number only if other programs stutter while the AI runs, or if the AI cannot keep up.",
            12,
            mode,
        ));
    }
    if let Some(rt) = &model.cache.runtime {
        col = col.push(tip(kv("AI engine", rt.clone(), mode), help::RUNTIME));
    }
    section_accent("AI", theme::INDIGO, mode, col)
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
