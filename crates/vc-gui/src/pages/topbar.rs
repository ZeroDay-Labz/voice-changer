//! Always visible: power, hear-myself, meters with gains, preset, microphone.

use crate::host::Host;
use crate::theme::{self, Mode};
use crate::widgets::{icons, knob::param_slider_compact, meter::meter};
use crate::{Element, Message, Model, tip};
use iced_core::{Alignment, Color, Length};
use iced_widget::{button, column, container, pick_list, row, text, toggler};
use nice_plug::prelude::*;

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let t = theme::tokens(mode);
    let p = host.params();
    let on = !p.bypass.value();

    let power = tip(
        button(
            row![
                icons::power(
                    22.0,
                    if on {
                        Color::from_rgb8(8, 30, 22)
                    } else {
                        t.text_dim
                    }
                ),
                text(if on { "ON" } else { "OFF" }).size(17)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .style(theme::power_button(on))
        .padding(crate::pad(12.0, 20.0, 12.0, 16.0))
        .on_press(Message::TogglePower),
        "",
    );

    let mut left = row![power].spacing(10).align_y(Alignment::Center);
    if let Some(mon) = host.monitor() {
        left = left.push(tip(
            button(
                row![
                    icons::headphones(
                        18.0,
                        if mon {
                            Color::from_rgb8(8, 30, 40)
                        } else {
                            t.text
                        }
                    ),
                    text("Hear myself").size(14)
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .style(if mon {
                theme::button_sky
            } else {
                theme::button_soft
            })
            .padding([10, 14])
            .on_press(Message::ToggleMonitor),
            "",
        ));
    }

    let meters = column![
        meter_line(
            "In",
            model.meters.input,
            model.meters.in_hold,
            !model.meters.gate_open,
            &p.input_gain,
            Some(&p.auto_level),
            mode
        ),
        meter_line(
            "Out",
            model.meters.output,
            model.meters.out_hold,
            !on,
            &p.output_gain,
            None,
            mode
        ),
    ]
    .spacing(6)
    .width(Length::FillPortion(3));

    let mut right = row![].spacing(10).align_y(Alignment::Center);
    let preset_name = model
        .current_preset
        .clone()
        .unwrap_or_else(|| "Custom".into());
    right = right.push(tip(
        container(
            row![icons::star(13.0, t.text_dim), text(preset_name).size(13)]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([6, 12])
        .style(theme::pill),
        "",
    ));
    if host.capabilities().microphone {
        right = right.push(mic_picker(model, host, 200.0, false));
    }

    container(
        row![left, meters, right]
            .spacing(18)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(crate::pad(14.0, 24.0, 10.0, 24.0))
    .width(Length::Fill)
    .into()
}

fn meter_line<'a>(
    label: &'a str,
    level: f32,
    hold: f32,
    muted: bool,
    gain: &'a FloatParam,
    auto: Option<&'a BoolParam>,
    mode: Mode,
) -> Element<'a> {
    let t = theme::tokens(mode);
    let mut r = row![
        container(text(label).size(12).color(t.text_dim)).width(Length::Fixed(26.0)),
        meter(level, hold, muted, mode, Length::Fill),
        container(text(peak_db(hold)).size(11).color(if hold >= 0.99 {
            theme::DANGER
        } else {
            t.text_dim
        }))
        .width(Length::Fixed(34.0)),
        param_slider_compact(gain, mode, 92.0),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(a) = auto {
        let ptr = a.as_ptr();
        r = r.push(tip(
            toggler(a.value())
                .label("Auto")
                .text_size(12)
                .size(16)
                .on_toggle(move |b| Message::ParamBool(ptr, b))
                .style(theme::toggler_style),
            "",
        ));
    }
    r.into()
}

fn peak_db(level: f32) -> String {
    if level <= 1e-4 {
        "−∞".into()
    } else {
        format!("{:.0}", 20.0 * level.log10())
    }
}

pub fn mic_picker<'a>(
    model: &'a Model,
    host: &'a dyn Host,
    width: f32,
    with_help: bool,
) -> Element<'a> {
    let max_chars = ((width - 40.0) / 7.0) as usize;
    let devices: Vec<(String, String)> = model
        .cache
        .devices
        .iter()
        .map(|d| (d.name.clone(), ellipsize(&d.description, max_chars)))
        .collect();
    let current = model.cache.capture_target.clone();
    let mut options: Vec<String> = vec!["System default".to_string()];
    options.extend(devices.iter().map(|(_, label)| label.clone()));
    let selected = current
        .as_ref()
        .and_then(|name| devices.iter().find(|(n, _)| n == name))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| "System default".to_string());
    let lookup = devices.clone();
    let picker = pick_list(options, Some(selected), move |label: String| {
        Message::SelectMic(
            lookup
                .iter()
                .find(|(_, l)| *l == label)
                .map(|(n, _)| n.clone()),
        )
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(width));
    tip(
        row![
            icons::mic(16.0, theme::tokens(host.theme_mode()).text_dim),
            picker
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        if with_help {
            crate::help::MIC_PICKER
        } else {
            ""
        },
    )
}

/// Latency / compute / dropouts summary for the sidebar.
pub fn status_pill<'a>(_model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let t = theme::tokens(mode);
    let mut lines = column![].spacing(2);
    let mut dot = theme::ACCENT;
    lines = lines.push(text(format!("{:.1} ms latency", host.latency_ms())).size(12));
    if let Some(stats) = host.stats()
        && stats.underruns > 0
    {
        dot = theme::WARN;
        lines = lines.push(
            text(format!("{} dropouts", stats.underruns))
                .size(12)
                .color(theme::WARN),
        );
    }
    #[cfg(feature = "ai")]
    if let Some(ai) = host.ai_status() {
        use vc_core::ai::AiState;
        match ai.state() {
            AiState::Off => {}
            AiState::Loading => {
                lines = lines.push(text("AI loading…").size(12).color(theme::ACCENT));
            }
            AiState::Error => {
                dot = theme::DANGER;
                lines = lines.push(text("AI error").size(12).color(theme::DANGER));
            }
            AiState::Ready => {
                let load = ai.load_factor();
                if load > 0.9 {
                    dot = theme::WARN;
                }
                lines = lines.push(
                    text(format!(
                        "AI {} · {:.0}%",
                        vc_core::ai::rvc::backend_name(),
                        load * 100.0
                    ))
                    .size(12)
                    .color(if load > 0.9 { theme::WARN } else { t.text_dim }),
                );
            }
        }
    }
    #[cfg(feature = "ai")]
    if let Some(ai) = host.ai_status()
        && ai.state() == vc_core::ai::AiState::Ready
        && host.params().ai_enabled.value()
    {
        let load = ai.load_factor().clamp(0.0, 1.0);
        lines = lines.push(iced_widget::progress_bar(0.0..=1.0, load).girth(3.0).style(
            move |theme| {
                let mut s = theme::progress_style(theme);
                s.bar = (if load > 0.9 {
                    theme::WARN
                } else {
                    theme::INDIGO
                })
                .into();
                s
            },
        ));
    }
    let body = container(
        row![
            container(
                iced_widget::space::horizontal()
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
            )
            .style(move |_t| container::Style {
                background: Some(dot.into()),
                border: theme::radius(4.0),
                ..container::Style::default()
            }),
            lines
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 10])
    .width(Length::Fill)
    .style(theme::pill);
    tip(
        button(body)
            .style(theme::button_ghost)
            .padding(0)
            .width(Length::Fill)
            .on_press(Message::Go(crate::Page::Settings)),
        crate::help::STATUS_PILL,
    )
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}
