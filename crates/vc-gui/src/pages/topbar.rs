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
    let caps = host.capabilities();
    if caps.microphone || caps.output {
        let mut pickers = column![].spacing(6);
        if caps.microphone {
            pickers = pickers.push(mic_picker(model, host, 200.0, true));
        }
        if caps.output {
            pickers = pickers.push(output_picker(model, host, 200.0, true));
        }
        right = right.push(pickers);
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

/// One entry of a device picker: the device's stable name (`None` = the
/// system default) and the text shown for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    pub name: Option<String>,
    label: String,
}

impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn device_choices(
    devices: &[crate::host::Device],
    current: &Option<String>,
    max_chars: usize,
) -> (Vec<DeviceChoice>, DeviceChoice) {
    let default = DeviceChoice {
        name: None,
        label: "System default".into(),
    };
    let mut options = vec![default.clone()];
    options.extend(devices.iter().map(|d| DeviceChoice {
        name: Some(d.name.clone()),
        label: ellipsize(&d.description, max_chars),
    }));
    let selected = match current {
        None => default,
        Some(name) => options
            .iter()
            .find(|o| o.name.as_ref() == Some(name))
            .cloned()
            // Chosen but unplugged: keep showing it, so the choice is not
            // silently replaced by "System default".
            .unwrap_or_else(|| DeviceChoice {
                name: Some(name.clone()),
                label: "Not connected".into(),
            }),
    };
    (options, selected)
}

pub fn mic_picker<'a>(
    model: &'a Model,
    host: &'a dyn Host,
    width: f32,
    with_help: bool,
) -> Element<'a> {
    let max_chars = ((width - 40.0) / 7.0) as usize;
    let (options, selected) =
        device_choices(&model.cache.devices, &model.cache.capture_target, max_chars);
    let picker = pick_list(options, Some(selected), |c: DeviceChoice| {
        Message::SelectMic(c.name)
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

/// Where the voice plays: the output device.
pub fn output_picker<'a>(
    model: &'a Model,
    host: &'a dyn Host,
    width: f32,
    with_help: bool,
) -> Element<'a> {
    let max_chars = ((width - 40.0) / 7.0) as usize;
    let (options, selected) =
        device_choices(&model.cache.outputs, &model.cache.output_target, max_chars);
    let picker = pick_list(options, Some(selected), |c: DeviceChoice| {
        Message::SelectOutput(c.name)
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(width));
    tip(
        row![
            icons::speaker(16.0, theme::tokens(host.theme_mode()).text_dim),
            picker
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        if with_help {
            if host.capabilities().monitor_toggle {
                crate::help::OUTPUT_PICKER_MONITOR
            } else {
                crate::help::OUTPUT_PICKER
            }
        } else {
            ""
        },
    )
}

/// Latency / compute / dropouts summary for the sidebar.
pub fn status_pill<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let t = theme::tokens(mode);
    let mut lines = column![].spacing(2);
    let mut dot = theme::ACCENT;
    if let Some(a) = &model.cache.audio {
        match a.state {
            crate::AudioState::Failed => {
                dot = theme::DANGER;
                lines = lines.push(text("Audio not running").size(12).color(theme::DANGER));
            }
            crate::AudioState::Reconnecting | crate::AudioState::Starting => {
                dot = theme::WARN;
                lines = lines.push(text("Reconnecting audio…").size(12).color(theme::WARN));
            }
            crate::AudioState::Running => {
                let line = match a.listeners {
                    0 => "No app listening".to_string(),
                    1 => "1 app listening".to_string(),
                    n => format!("{n} apps listening"),
                };
                lines = lines.push(text(line).size(12).color(if a.listeners == 0 {
                    t.text_dim
                } else {
                    theme::SKY
                }));
            }
        }
    }
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
