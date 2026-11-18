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
    col = col.push(appearance(model, host, mode));
    if caps.standalone {
        col = col.push(logs(host, mode));
    }
    col = col.push(about(host, mode));
    col.into()
}

fn labelled<'a>(label: &'a str, mode: Mode, content: impl Into<Element<'a>>) -> Element<'a> {
    let t = theme::tokens(mode);
    row![
        container(text(label).size(14).color(t.text_dim))
            .width(Length::Fixed(crate::widgets::knob::LABEL_WIDTH)),
        content.into()
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn audio<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let caps = host.capabilities();
    let mut col = column![].spacing(10);
    if caps.microphone {
        col = col.push(labelled(
            "Microphone",
            mode,
            super::topbar::mic_picker(model, host, 320.0, true),
        ));
    }
    if caps.output {
        col = col.push(labelled(
            if caps.monitor_toggle {
                "Hear myself on"
            } else {
                "Output"
            },
            mode,
            super::topbar::output_picker(model, host, 320.0, true),
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
        col = col.push(dim("Plays the changed voice on the output above so you can hear what others hear. Use headphones.", 12, mode));
    } else if caps.output {
        col = col.push(dim(
            "Your changed voice plays on this output: headphones, speakers, a mixer or an audio interface. Nothing else on the computer is changed.",
            12,
            mode,
        ));
    }
    if let Some(a) = &model.cache.audio {
        let (line, color) = match a.state {
            crate::AudioState::Running => (
                if !caps.routing {
                    "Running.".to_string()
                } else if a.virtual_mic {
                    "Running. Other programs can pick “Voice Changer Mic” as their microphone."
                        .to_string()
                } else {
                    "Running, but “Voice Changer Mic” is not ready yet.".to_string()
                },
                theme::ink(theme::ACCENT, mode),
            ),
            crate::AudioState::Starting => ("Starting…".to_string(), theme::tokens(mode).text_dim),
            crate::AudioState::Reconnecting => (
                if a.message.is_empty() {
                    "Reconnecting to your sound devices…".to_string()
                } else {
                    a.message.clone()
                },
                theme::ink(theme::WARN, mode),
            ),
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
                    .color(theme::ink(theme::WARN, mode))
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
        if a.state == crate::AudioState::Failed {
            col = col.push(
                button(text("Try again").size(13))
                    .style(theme::button_primary)
                    .padding([6, 12])
                    .on_press(Message::RetryAudio),
            );
        }
        if let (Some(active), Some(chosen)) = (&a.active_mic, &a.chosen_mic)
            && active != chosen
        {
            col = col.push(
                text(format!(
                    "Using {active} for now: {chosen} is not connected. It is used again as soon as it is back."
                ))
                .size(12)
                .color(theme::ink(theme::WARN, mode))
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
    }
    col = col.push(kv("Delay", super::topbar::delay_value(host), mode));
    if let Some(stats) = host.stats() {
        col = col.push(kv(
            "Dropouts",
            if stats.underruns == 0 {
                "none".to_string()
            } else {
                format!("{} since start (short gaps or crackle)", stats.underruns)
            },
            mode,
        ));
        if let Some(buffer) = model.cache.settings.buffer {
            let choices: [(u32, &str); 4] = [
                (128, "Small: least delay"),
                (0, "Normal"),
                (512, "Large: steadier"),
                (1024, "Largest: for a busy computer"),
            ];
            let labels: Vec<&'static str> = choices.iter().map(|(_, l)| *l).collect();
            let selected = choices
                .iter()
                .find(|(frames, _)| *frames == buffer || (*frames == 0 && buffer == 256))
                .map(|(_, l)| *l);
            let picker = pick_list(labels, selected, move |label: &'static str| {
                let frames = choices
                    .iter()
                    .find(|(_, l)| *l == label)
                    .map(|(f, _)| *f)
                    .unwrap_or(0);
                Message::Setting(HostSetting::Buffer(frames))
            })
            .style(theme::pick_list_style)
            .text_size(13)
            .width(Length::Fixed(260.0));
            col = col.push(labelled("Buffer", mode, tip(picker, help::BUFFER)));
            let running = (stats.quantum_us as f32 * 48.0 / 1000.0).round() as u32;
            let wanted = if buffer == 0 { 256 } else { buffer };
            if running != wanted {
                col = col.push(dim(
                    "The new buffer size is used the next time Voice Changer starts.",
                    12,
                    mode,
                ));
            } else if stats.underruns > 20 && buffer < 1024 {
                col = col.push(dim(
                    "If you hear crackle, pick a larger buffer and start Voice Changer again.",
                    12,
                    mode,
                ));
            }
        }
    }
    section_accent("Sound", theme::SKY, mode, col)
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
            "Start with the computer",
            mode,
            tip(
                toggler(auto)
                    .on_toggle(|b| Message::Setting(HostSetting::Autostart(b)))
                    .style(theme::toggler_style),
                help::AUTOSTART,
            ),
        ));
    }
    let mut out = column![section("Window", mode, col)].spacing(16);
    out = out.push(shortcuts(host, mode));
    out.into()
}

/// A key combination as a little key cap.
fn keys<'a>(combo: impl Into<String>) -> Element<'a> {
    container(
        text(combo.into())
            .size(12)
            .font(iced_core::Font::MONOSPACE)
            .wrapping(iced_core::text::Wrapping::None),
    )
    .padding([3, 8])
    .style(theme::raised)
    .into()
}

/// Every shortcut in one place: the ones that work from any program and
/// the ones inside the window.
fn shortcuts<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let mut col = column![].spacing(8);
    if let Some(hotkey) = host.hotkey() {
        let current: Element<'a> = match hotkey {
            Some(k) if !k.is_empty() => keys(k),
            Some(_) => dim("not set", 13, mode).into(),
            None => dim("not available on this desktop", 13, mode).into(),
        };
        let available = host.hotkey().is_some_and(|h| h.is_some());
        let mut line = row![current].spacing(8).align_y(Alignment::Center);
        if host.capabilities().hotkey_config && available {
            line = line.push(tip(
                button(text("Change…").size(13).color(t.text))
                    .style(theme::button_soft)
                    .padding([6, 10])
                    .on_press(Message::ConfigureHotkey),
                help::HOTKEY,
            ));
        }
        col = col.push(dim("From any program", 12, mode));
        col = col.push(labelled("Voice changer on/off", mode, line));
        let mute: Element<'a> = match host.settings().mute_hotkey {
            Some(k) if !k.is_empty() => keys(k),
            _ => dim("not set", 13, mode).into(),
        };
        col = col.push(labelled("Mute", mode, mute));
    }
    col = col.push(dim("In this window", 12, mode));
    let mut list: Vec<(&str, &str)> = vec![("Voice changer on/off", "Ctrl+M"), ("Mute", "Ctrl+U")];
    if host.monitor().is_some() {
        list.push(("Hear myself", "Ctrl+H"));
    }
    list.extend([
        ("Save as a preset", "Ctrl+S"),
        ("Search presets", "Ctrl+F"),
        ("Go to a page", "Ctrl+1 … Ctrl+5"),
        ("Cancel, clear the search", "Esc"),
        ("Quit", "Ctrl+Q"),
    ]);
    for (what, combo) in list {
        col = col.push(labelled(what, mode, keys(combo)));
    }
    col = col.push(dim(
        "Knobs and sliders: drag, roll the mouse wheel, hold Shift for fine steps, double-click to reset.",
        12,
        mode,
    ));
    section_with("Shortcuts", icons::keyboard(18.0, t.text_dim), mode, col)
}

fn appearance<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    use crate::theme::Choice;
    let standalone = host.capabilities().standalone;
    let mut choices: Vec<(Choice, &'static str)> =
        vec![(Choice::Dark, "Dark"), (Choice::Light, "Light")];
    if standalone {
        choices.push((Choice::System, "Same as the desktop"));
    }
    let current = model.cache.settings.theme;
    let labels: Vec<&'static str> = choices.iter().map(|(_, l)| *l).collect();
    let selected = choices
        .iter()
        .find(|(c, _)| *c == current)
        .map(|(_, l)| *l)
        .or(Some(match mode {
            Mode::Dark => "Dark",
            Mode::Light => "Light",
        }));
    let picker = pick_list(labels, selected, move |label: &'static str| {
        Message::Setting(HostSetting::Theme(
            choices
                .iter()
                .find(|(_, l)| *l == label)
                .map(|(c, _)| *c)
                .unwrap_or_default(),
        ))
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(200.0));
    let mut col = column![labelled("Theme", mode, tip(picker, help::THEME))].spacing(10);
    if let Some(scale) = model.cache.settings.ui_scale {
        let sizes: [(f32, &str); 6] = [
            (0.85, "Smaller"),
            (1.0, "Normal"),
            (1.1, "A little larger"),
            (1.25, "Larger"),
            (1.4, "Much larger"),
            (1.5, "Largest"),
        ];
        let labels: Vec<&'static str> = sizes.iter().map(|(_, l)| *l).collect();
        let selected = sizes
            .iter()
            .min_by(|a, b| (a.0 - scale).abs().total_cmp(&(b.0 - scale).abs()))
            .map(|(_, l)| *l);
        let picker = pick_list(labels, selected, move |label: &'static str| {
            Message::Setting(HostSetting::UiScale(
                sizes
                    .iter()
                    .find(|(_, l)| *l == label)
                    .map(|(s, _)| *s)
                    .unwrap_or(1.0),
            ))
        })
        .style(theme::pick_list_style)
        .text_size(13)
        .width(Length::Fixed(200.0));
        col = col.push(labelled("Size", mode, tip(picker, help::UI_SCALE)));
    }
    if standalone && !model.cache.settings.first_run {
        col = col.push(labelled(
            "First steps",
            mode,
            tip(
                button(text("Show them again").size(13))
                    .style(theme::button_soft)
                    .padding([6, 10])
                    .on_press(Message::Setting(HostSetting::ShowWelcome)),
                help::SHOW_WELCOME,
            ),
        ));
    }
    section("Appearance", mode, col)
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
    let copy = tip(
        button(text("Copy problem report").size(13))
            .style(theme::button_soft)
            .padding([6, 10])
            .on_press(Message::CopyReport),
        help::COPY_REPORT,
    );
    section_with(
        "Log",
        row![copy, open].spacing(8).align_y(Alignment::Center),
        mode,
        body,
    )
}

fn about<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let caps = host.capabilities();
    let mut col = column![
        kv("Version", env!("CARGO_PKG_VERSION"), mode),
        kv(
            "Running as",
            if !caps.standalone {
                "a plugin inside another program"
            } else if caps.routing {
                "its own program, with a virtual microphone for other programs"
            } else {
                "its own program, playing on the output you picked"
            },
            mode
        ),
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
