//! Mixer: gains, cleanup, gate, limiter, and where the voice goes.

use super::{dim, kv, section_accent};
use crate::help;
use crate::host::Host;
use crate::theme::{self, Mode};
use crate::widgets::icons;
use crate::widgets::knob::{knob, param_slider, param_toggle};
use crate::{Element, Message, Model, tip};
use iced_core::{Alignment, Length};
use iced_widget::{button, checkbox, column, container, row, text, toggler};

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let p = host.params();
    let auto_db = host
        .meters()
        .auto_gain_db
        .load(std::sync::atomic::Ordering::Relaxed);
    let levels = column![
        container(
            row![
                knob(&p.input_gain, Some("Input"), mode, 92.0, theme::SKY),
                knob(&p.output_gain, Some("Output"), mode, 92.0, theme::SKY),
            ]
            .spacing(28)
        )
        .center_x(Length::Fill),
        dim(
            "Drag a knob up or down, roll the mouse wheel over it, or double-click to reset.",
            12,
            mode
        ),
        param_toggle(&p.auto_level, mode),
        kv("Auto gain now", format!("{auto_db:+.1} dB"), mode),
        param_toggle(&p.limiter, mode),
    ]
    .spacing(10);
    let cleanup = column![
        param_toggle(&p.denoise, mode),
        param_toggle(&p.voice_only, mode),
        param_slider(&p.gate_floor, mode, 220.0),
        param_toggle(&p.gate_enabled, mode),
        param_slider(&p.gate_threshold, mode, 220.0),
        dim("Noise suppression and the voice-only gate run before the voice engine, so background noise never becomes voice.", 12, mode),
    ]
    .spacing(10);
    let mut col = column![
        row![
            section_accent("Levels", theme::SKY, mode, levels),
            section_accent("Cleanup", theme::AMBER, mode, cleanup)
        ]
        .spacing(16)
        .wrap()
    ]
    .spacing(16);
    if host.capabilities().standalone {
        col = col.push(send_to(model, mode));
    }
    col.into()
}

/// Where the processed voice goes: the default microphone for everything,
/// and/or specific recording applications.
pub fn send_to<'a>(model: &'a Model, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let is_default = model.cache.default_source.unwrap_or(false);
    let mut col = column![
        tip(
            row![
                toggler(is_default).on_toggle(Message::SetDefaultSource).style(theme::toggler_style),
                column![
                    text("Replace my microphone everywhere").size(14),
                    dim("Switches every app that reads your microphone over to the voice changer, including apps pinned to a specific device. Everything returns to normal when you turn this off or quit.", 12, mode)
                ]
                .spacing(2)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            help::DEFAULT_SOURCE
        ),
        row![
            text("Send to these applications").size(14),
            iced_widget::space::horizontal(),
            tip(button(icons::refresh(14.0, t.text_dim)).style(theme::button_ghost).padding(6).on_press(Message::RefreshApps), help::REFRESH_APPS)
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(10);
    if model.cache.app_streams.is_empty() {
        col = col.push(dim("No application is recording right now. Apps appear here once they open their microphone (Discord: join a call; OBS: add a mic source).", 12, mode));
    }
    for s in &model.cache.app_streams {
        let id = s.id;
        let label = if s.media.is_empty() || s.media == s.app {
            s.app.clone()
        } else {
            format!("{} · {}", s.app, s.media)
        };
        let line = row![
            checkbox(s.routed)
                .label(label)
                .text_size(14)
                .on_toggle(move |on| Message::RouteApp(id, on)),
            iced_widget::space::horizontal(),
        ]
        .align_y(Alignment::Center);
        let line: Element<'a> = if s.routed {
            row![line, super::badge("gets the voice", theme::SKY)]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        } else {
            line.into()
        };
        col = col.push(tip(
            container(line)
                .padding([6, 10])
                .width(Length::Fill)
                .style(theme::raised),
            help::ROUTE_APP,
        ));
    }
    col = col.push(dim(
        "Any app can also just pick “Voice Changer Mic” in its own input settings.",
        12,
        mode,
    ));
    section_accent("Send the voice to", theme::SKY, mode, col)
}
