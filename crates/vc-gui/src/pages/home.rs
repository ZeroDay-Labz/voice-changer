//! Home: presets, quick tweaks and the AI voice card, laid out to fit one
//! window without scrolling.

#[cfg(feature = "ai")]
use super::badge;
use super::{dim, section_accent, section_with};
use crate::help;
use crate::host::Host;
use crate::theme::{self, Mode};
use crate::widgets::{icons, knob::knob};
use crate::{Element, Message, Model, tip};
use iced_core::{Alignment, Length};
use iced_widget::{button, column, container, grid, row, text, text_input};
use nice_plug::prelude::*;
use vc_core::PresetEntry;

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let lower = row![quick_tweaks(host, mode)].spacing(16);
    #[cfg(feature = "ai")]
    let lower = lower.push(container(ai_card(model, host, mode)).width(Length::Fill));
    let mut col = column![].spacing(14);
    if let Some(note) = nothing_changing(host) {
        col = col.push(notice(note, mode));
    }
    col = col.push(presets(model, mode));
    col = col.push(lower.wrap());
    col.into()
}

/// Why the voice would sound unchanged right now, if that is the case.
fn nothing_changing(host: &dyn Host) -> Option<&'static str> {
    let p = host.params();
    if p.bypass.value() {
        return None;
    }
    #[cfg(feature = "ai")]
    if p.ai_enabled.value() && !p.ai_voice().is_empty() {
        return None;
    }
    p.voice_is_neutral().then_some(
        "Nothing is changing your voice yet — pick a preset below, or turn a Quick tweak knob.",
    )
}

/// A one-line amber banner.
fn notice<'a>(text_: &'a str, _mode: Mode) -> Element<'a> {
    container(
        row![
            icons::star(14.0, theme::AMBER),
            text(text_).size(13).color(theme::AMBER)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::panel_accent(theme::AMBER))
    .into()
}

fn presets<'a>(model: &'a Model, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let q = model.search.trim().to_lowercase();
    let mut entries: Vec<&PresetEntry> = model
        .presets
        .iter()
        .filter(|e| {
            q.is_empty()
                || e.preset.name.to_lowercase().contains(&q)
                || e.preset.description.to_lowercase().contains(&q)
        })
        .collect();
    entries.sort_by_key(|e| !model.favorites.contains(&e.preset.name));

    let mut cards = grid![]
        .spacing(8)
        .fluid(150)
        .height(grid::aspect_ratio(150, 74));
    for e in entries {
        let selected = model.current_preset.as_deref() == Some(e.preset.name.as_str());
        let fav = model.favorites.contains(&e.preset.name);
        let name = e.preset.name.clone();
        let stripe = category_color(&e.preset.name);
        let star = tip(
            button(icons::star(
                13.0,
                if fav { theme::AMBER } else { t.text_dim },
            ))
            .style(theme::button_ghost)
            .padding(3)
            .on_press(Message::ToggleFavorite(name.clone())),
            help::FAVOURITE,
        );
        let mut top = row![
            text(e.preset.name.as_str()).size(14),
            iced_widget::space::horizontal(),
            star
        ]
        .align_y(Alignment::Center);
        if !e.is_factory() {
            top = top.push(tip(
                button(icons::trash(13.0, t.text_dim))
                    .style(theme::button_ghost)
                    .padding(3)
                    .on_press(Message::DeletePreset(name.clone())),
                help::DELETE_PRESET,
            ));
        }
        let body = row![
            container(iced_widget::space::vertical())
                .width(Length::Fixed(3.0))
                .height(Length::Fill)
                .style(move |_t| container::Style {
                    background: Some(stripe.into()),
                    border: theme::radius(2.0),
                    ..container::Style::default()
                }),
            column![top, dim(short(&e.preset.description, 44), 11, mode)]
                .spacing(2)
                .width(Length::Fill),
        ]
        .spacing(8)
        .height(Length::Fixed(58.0));
        let card = button(body)
            .width(Length::Fill)
            .padding(crate::pad(8.0, 8.0, 8.0, 8.0))
            .style(move |theme, status| {
                let t = theme::tokens(theme::mode_of(theme));
                let hovered = matches!(
                    status,
                    iced_widget::button::Status::Hovered | iced_widget::button::Status::Pressed
                );
                iced_widget::button::Style {
                    background: Some(
                        (if selected {
                            theme::tint(stripe, 0.22)
                        } else if hovered {
                            t.raised_hover
                        } else {
                            t.raised
                        })
                        .into(),
                    ),
                    text_color: t.text,
                    border: iced_core::Border {
                        radius: iced_core::border::radius(10),
                        width: if selected { 1.5 } else { 0.0 },
                        color: stripe,
                    },
                    ..iced_widget::button::Style::default()
                }
            })
            .on_press(Message::ApplyPreset(name));
        cards = cards.push(tip(card, e.preset.description.as_str()));
    }

    let search = tip(
        text_input("Search presets…", &model.search)
            .id(crate::search_id())
            .on_input(Message::SearchChanged)
            .size(13)
            .style(theme::text_input_style)
            .width(Length::Fixed(200.0)),
        help::SEARCH,
    );

    let save: Element<'a> = if model.saving {
        row![
            text_input("Preset name", &model.save_name)
                .on_input(Message::SaveNameChanged)
                .on_submit(Message::SavePreset)
                .size(13)
                .style(theme::text_input_style)
                .width(Length::Fixed(180.0)),
            button(text("Save").size(13))
                .style(theme::button_primary)
                .padding([8, 12])
                .on_press(Message::SavePreset),
            button(text("Cancel").size(13))
                .style(theme::button_ghost)
                .padding([8, 12])
                .on_press(Message::CancelSave),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else {
        tip(
            button(
                row![icons::plus(14.0, t.text), text("Save current").size(13)]
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .style(theme::button_soft)
            .padding([8, 12])
            .on_press(Message::StartSave),
            help::SAVE_PRESET,
        )
    };

    section_with(
        "Presets",
        row![search, save].spacing(10).align_y(Alignment::Center),
        mode,
        cards,
    )
}

/// A colour per preset family so the grid is not one flat tone.
fn category_color(name: &str) -> iced_core::Color {
    match name {
        "Natural" | "Announcer" => theme::ACCENT,
        "Man" | "Woman" | "Baritone" | "Bass" | "Deep" => theme::SKY,
        "Demon" | "Monster" => theme::CORAL,
        "Chipmunk" | "Robot" => theme::VIOLET,
        "Radio" | "Cave" => theme::AMBER,
        _ => theme::INDIGO,
    }
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

fn quick_tweaks<'a>(host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let p = host.params();
    let knobs = row![
        knob(&p.pitch, Some("Pitch"), mode, 76.0, theme::ACCENT),
        knob(&p.formant, Some("Character"), mode, 76.0, theme::SKY),
        knob(&p.drive, Some("Grit"), mode, 76.0, theme::AMBER),
        knob(&p.reverb_mix, Some("Space"), mode, 76.0, theme::INDIGO),
        knob(&p.echo_mix, Some("Echo"), mode, 76.0, theme::VIOLET),
    ]
    .spacing(14);
    container(section_accent(
        "Quick tweaks",
        theme::ACCENT,
        mode,
        container(knobs).center_x(Length::Fill),
    ))
    .width(Length::Fixed(500.0))
    .into()
}

#[cfg(feature = "ai")]
fn ai_card<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    use crate::widgets::knob::{param_slider, param_toggle};
    use vc_core::ai::AiState;

    let t = theme::tokens(mode);
    let p = host.params();
    let current = p.ai_voice();
    let current_name = model
        .voices
        .iter()
        .find(|v| v.path.to_string_lossy() == current)
        .map(|v| v.name.clone());

    // Voice picker.
    let mut options: Vec<String> = vec!["None".into()];
    options.extend(model.voices.iter().map(|v| v.name.clone()));
    let lookup: Vec<(String, String)> = model
        .voices
        .iter()
        .map(|v| (v.name.clone(), v.path.to_string_lossy().to_string()))
        .collect();
    let picker = iced_widget::pick_list(
        options,
        Some(current_name.clone().unwrap_or_else(|| {
            if current.is_empty() {
                "None".into()
            } else {
                "(missing file)".into()
            }
        })),
        move |label: String| {
            Message::SelectVoice(
                lookup
                    .iter()
                    .find(|(n, _)| *n == label)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default(),
            )
        },
    )
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(210.0));

    let ai_ptr = p.ai_enabled.as_ptr();
    let enable = iced_widget::toggler(p.ai_enabled.value())
        .label("Use AI voice")
        .text_size(13)
        .on_toggle_maybe((!current.is_empty()).then_some(move |b| Message::ParamBool(ai_ptr, b)))
        .style(theme::toggler_style);

    let mut left = column![
        row![
            container(text("Voice").size(13).color(t.text_dim)).width(Length::Fixed(100.0)),
            tip(picker, help::VOICE_PICKER),
            tip(
                button(icons::refresh(13.0, t.text_dim))
                    .style(theme::button_ghost)
                    .padding(5)
                    .on_press(Message::RefreshLibrary),
                help::RESCAN
            )
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            container(iced_widget::space::horizontal()).width(Length::Fixed(100.0)),
            tip(enable, help::param("AI Voice"))
        ]
        .spacing(8),
        param_slider(&p.ai_pitch, mode, 150.0),
        param_slider(&p.ai_breath, mode, 150.0),
        param_toggle(&p.ai_solo, mode),
    ]
    .spacing(6);

    // Multi-speaker models: pick the speaker (instant, no reload).
    if let Some(v) = model
        .voices
        .iter()
        .find(|v| v.path.to_string_lossy() == current && v.speakers > 1)
    {
        let labels: Vec<String> = (0..v.speakers).map(|i| v.speaker_label(i)).collect();
        let cur = (p.ai_speaker.value().max(0) as u32).min(v.speakers - 1);
        let ptr = p.ai_speaker.as_ptr();
        let n = v.speakers;
        let labels2 = labels.clone();
        let picker =
            iced_widget::pick_list(labels, Some(v.speaker_label(cur)), move |label: String| {
                let i = labels2.iter().position(|l| *l == label).unwrap_or(0) as f32;
                Message::ParamJump(ptr, i / 15.0)
            })
            .style(theme::pick_list_style)
            .text_size(13)
            .width(Length::Fixed(210.0));
        let _ = n;
        left = left.push(
            row![
                container(text("Speaker").size(13).color(t.text_dim)).width(Length::Fixed(100.0)),
                tip(picker, help::param("AI Speaker"))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    let has_index = host
        .ai_status()
        .is_some_and(|s| s.has_index.load(std::sync::atomic::Ordering::Relaxed))
        || model
            .voices
            .iter()
            .any(|v| v.path.to_string_lossy() == current && v.has_index);
    if has_index {
        left = left.push(param_toggle(&p.ai_index_enabled, mode));
        if p.ai_index_enabled.value() {
            left = left.push(param_slider(&p.ai_index_rate, mode, 150.0));
        }
    }

    // Compute picker (options snapshotted in the cache).
    let device = p.ai_device();
    let copts = model.cache.compute_options.clone();
    let labels: Vec<String> = copts.iter().map(|(_, l)| l.clone()).collect();
    let selected = copts
        .iter()
        .find(|(k, _)| *k == device)
        .map(|(_, l)| l.clone())
        .or_else(|| labels.first().cloned());
    let compute_pick = iced_widget::pick_list(labels, selected, move |label: String| {
        Message::SelectDevice(
            copts
                .iter()
                .find(|(_, l)| *l == label)
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "auto".into()),
        )
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(250.0));

    let mut right = column![
        row![
            icons::chip(15.0, theme::INDIGO),
            text("Compute").size(13).color(t.text_dim)
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        tip(compute_pick, help::COMPUTE),
        super::enum_picker(&p.ai_speed, mode, 250.0),
    ]
    .spacing(8)
    .width(Length::Fixed(400.0));
    if let Some(n) = &model.cache.gpu_note {
        right = right.push(dim(n.clone(), 11, mode));
    }

    // Status line.
    let (status_text, color): (String, iced_core::Color) = match host.ai_status() {
        None => ("AI not available in this host.".into(), t.text_dim),
        Some(st) => match st.state() {
            AiState::Off => {
                if model.voices.is_empty() {
                    (
                        "No voice models yet. Add one on the Voices page.".into(),
                        t.text_dim,
                    )
                } else {
                    (
                        "Pick a voice model to enable AI conversion.".into(),
                        t.text_dim,
                    )
                }
            }
            AiState::Loading => (format!("Loading… {}", st.message()), theme::INDIGO),
            AiState::Error => (format!("Error: {}", st.message()), theme::DANGER),
            AiState::Ready => {
                let infer = st.infer_ms.load(std::sync::atomic::Ordering::Relaxed);
                let load = st.load_factor();
                let drops = st.dropouts.load(std::sync::atomic::Ordering::Relaxed);
                let f0 = st.f0_hz.load(std::sync::atomic::Ordering::Relaxed);
                let warming = st.warming.load(std::sync::atomic::Ordering::Relaxed);
                let mut s = if p.ai_enabled.value() && warming {
                    format!(
                        "Warming up on {} — your live voice is passing through until the model catches up",
                        vc_core::ai::rvc::backend_name()
                    )
                } else if p.ai_enabled.value() {
                    format!(
                        "Ready on {} · {infer:.0} ms per block ({:.0}% of realtime)",
                        vc_core::ai::rvc::backend_name(),
                        load * 100.0
                    )
                } else {
                    format!(
                        "Ready on {} (switched off)",
                        vc_core::ai::rvc::backend_name()
                    )
                };
                if f0 > 0.0 && p.ai_enabled.value() {
                    s.push_str(&format!(" · voice ≈ {f0:.0} Hz"));
                }
                if drops > 0 {
                    s.push_str(&format!(" · {drops} dropouts"));
                }
                let colour = if warming && p.ai_enabled.value() {
                    theme::AMBER
                } else if load > 0.9 {
                    theme::WARN
                } else {
                    t.text_dim
                };
                (s, colour)
            }
        },
    };

    let state_badge = match host.ai_status().map(|s| s.state()) {
        Some(AiState::Ready) if p.ai_enabled.value() => badge("ACTIVE", theme::INDIGO),
        Some(AiState::Ready) => badge("READY", t.text_dim),
        Some(AiState::Loading) => badge("LOADING", theme::AMBER),
        Some(AiState::Error) => badge("ERROR", theme::DANGER),
        _ => badge("OFF", t.text_dim),
    };
    let manage = tip(
        button(text("Manage voices").size(13))
            .style(theme::button_soft)
            .padding([6, 10])
            .on_press(Message::Go(crate::Page::Voices)),
        help::MANAGE_VOICES,
    );

    let body = column![
        row![left, right].spacing(20).wrap(),
        text(status_text)
            .size(12)
            .color(color)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    ]
    .spacing(8);
    let t2 = theme::tokens(mode);
    container(
        column![
            row![
                text("AI voice").size(16).color(t2.text),
                iced_widget::space::horizontal(),
                state_badge,
                manage
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            body
        ]
        .spacing(10),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::panel_accent(theme::INDIGO))
    .into()
}
