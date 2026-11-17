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
    let mut col = column![].spacing(14);
    if let Some(card) = audio_card(model, mode) {
        col = col.push(card);
    }
    if model.cache.settings.first_run && host.capabilities().standalone {
        col = col.push(welcome(model, host, mode));
    }
    if let Some(note) = nothing_changing(host) {
        col = col.push(notice(note, mode));
    }
    col = col.push(presets(model, host, mode));
    col = col.push(quick_tweaks(host, mode));
    #[cfg(feature = "ai")]
    {
        col = col.push(ai_card(model, host, mode));
    }
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

/// Audio trouble, said plainly, with the one button that helps.
fn audio_card<'a>(model: &'a Model, mode: Mode) -> Option<Element<'a>> {
    let a = model.cache.audio.as_ref()?;
    let t = theme::tokens(mode);
    match a.state {
        crate::AudioState::Failed => {
            let body = column![
                text("Audio is not running").size(16).color(theme::DANGER),
                text(a.message.as_str())
                    .size(13)
                    .color(t.text)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                row![
                    tip(
                        button(text("Try again").size(13))
                            .style(theme::button_primary)
                            .padding([8, 14])
                            .on_press(Message::RetryAudio),
                        help::RETRY_AUDIO
                    ),
                    button(text("See the log").size(13))
                        .style(theme::button_soft)
                        .padding([8, 14])
                        .on_press(Message::Go(crate::Page::Settings)),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ]
            .spacing(8);
            Some(
                container(body)
                    .padding(16)
                    .width(Length::Fill)
                    .style(theme::panel_accent(theme::DANGER))
                    .into(),
            )
        }
        crate::AudioState::Reconnecting | crate::AudioState::Starting => Some(notice(
            if a.message.is_empty() {
                "Connecting to your sound devices… your settings are kept and the voice comes back by itself."
            } else {
                a.message.as_str()
            },
            mode,
        )),
        crate::AudioState::Running => {
            if !a.message.is_empty() {
                return Some(notice(a.message.as_str(), mode));
            }
            if !a.mic_alive {
                return Some(notice(
                    "No sound is arriving from the microphone. Check that it is plugged in and not switched off, or pick another one at the top right.",
                    mode,
                ));
            }
            None
        }
    }
}

/// First-run card: the few things a new user needs, each ticked off from
/// what is really happening.
fn welcome<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let caps = host.capabilities();
    let indent = |e: Element<'a>| container(e).padding(crate::pad(0.0, 0.0, 0.0, 28.0));
    let head = |n: usize, title: &'a str, done: bool| -> Element<'a> {
        row![
            container(if done {
                icons::check(16.0, theme::ACCENT)
            } else {
                text(n.to_string()).size(13).color(t.text_dim).into()
            })
            .width(Length::Fixed(20.0)),
            text(title).size(14).color(t.text),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    };
    let p = host.params();
    let mut steps: Vec<Element<'a>> = Vec::new();

    // 1. The microphone, with proof that it hears something.
    let hearing = model.meters.in_hold > 0.01;
    steps.push(
        column![
            head(1, "Your microphone", hearing),
            indent(super::topbar::mic_picker(model, host, 250.0, true)),
            indent(
                crate::widgets::meter::meter(
                    model.meters.input,
                    model.meters.in_hold,
                    false,
                    model.meters.in_clipped(),
                    false,
                    mode,
                    Length::Fixed(250.0)
                )
            ),
            indent(
                dim(
                    if hearing {
                        "It hears you."
                    } else {
                        "Say something: the bar should move. If it does not, pick another microphone."
                    },
                    12,
                    mode
                )
                .into()
            ),
        ]
        .spacing(6)
        .into(),
    );

    // 2. Where the changed voice is heard.
    if caps.output {
        let monitor = host.monitor();
        let mut col = column![
            head(
                2,
                if caps.monitor_toggle {
                    "Hear yourself"
                } else {
                    "Where your voice plays"
                },
                monitor.unwrap_or(true)
                    && model
                        .cache
                        .audio
                        .as_ref()
                        .is_some_and(|a| a.state == crate::AudioState::Running),
            ),
            indent(super::topbar::output_picker(model, host, 250.0, true)),
        ]
        .spacing(6);
        if let Some(on) = monitor {
            col = col.push(indent(
                button(
                    row![
                        icons::headphones(
                            16.0,
                            if on {
                                iced_core::Color::from_rgb8(8, 30, 40)
                            } else {
                                t.text
                            }
                        ),
                        text(if on {
                            "Hear myself is on"
                        } else {
                            "Turn on Hear myself"
                        })
                        .size(13)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .style(if on {
                    theme::button_sky
                } else {
                    theme::button_soft
                })
                .padding([8, 12])
                .on_press(Message::ToggleMonitor)
                .into(),
            ));
            col = col.push(indent(
                dim("Use headphones, or you will hear an echo.", 12, mode).into(),
            ));
        } else {
            col = col.push(indent(
                dim(
                    "Your changed voice comes out here: headphones, a mixer or an audio interface.",
                    12,
                    mode,
                )
                .into(),
            ));
        }
        steps.push(col.into());
    }

    // 3. A voice.
    #[allow(unused_mut)]
    let mut changed = !p.voice_is_neutral();
    #[cfg(feature = "ai")]
    {
        changed |= p.ai_enabled.value() && !p.ai_voice().is_empty();
    }
    steps.push(
        column![
            head(steps.len() + 1, "Pick a voice", changed),
            indent(
                dim(
                    "Click a preset below and talk. The ON button at the top left switches between the changed voice and your own.",
                    12,
                    mode
                )
                .into()
            ),
        ]
        .spacing(6)
        .into(),
    );

    // 4. Other programs (where there is a virtual microphone).
    if caps.routing {
        let listening = model.cache.audio.as_ref().is_some_and(|a| a.listeners > 0);
        steps.push(
            column![
                head(steps.len() + 1, "Use it in your apps", listening),
                indent(
                    dim(
                        "Pick “Voice Changer Mic” as the microphone in Discord, OBS or your browser. Or let the Mixer page switch every app over for you.",
                        12,
                        mode,
                    )
                    .into()
                ),
                indent(
                    button(text("Open the Mixer page").size(13))
                        .style(theme::button_soft)
                        .padding([8, 12])
                        .on_press(Message::Go(crate::Page::Mixer))
                        .into()
                ),
            ]
            .spacing(6)
            .into(),
        );
    }

    let mut cells = row![].spacing(20);
    for step in steps {
        cells = cells.push(container(step).width(Length::FillPortion(1)));
    }
    let header = row![
        text("First steps").size(16).color(t.text),
        iced_widget::space::horizontal(),
        tip(
            button(text("Got it").size(13))
                .style(theme::button_soft)
                .padding([6, 10])
                .on_press(Message::DismissWelcome),
            help::WELCOME_DONE
        )
    ]
    .align_y(Alignment::Center);
    container(column![header, cells].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(theme::panel_accent(theme::ACCENT))
        .into()
}

/// A one-line amber banner.
fn notice<'a>(text_: &'a str, mode: Mode) -> Element<'a> {
    container(
        row![
            icons::info(16.0, theme::AMBER),
            text(text_)
                .size(13)
                .color(theme::tokens(mode).text)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::panel_accent(theme::AMBER))
    .into()
}

fn presets<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
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
    let editing = model.editing_presets;
    let found = !entries.is_empty();

    let mut cards = grid![]
        .spacing(8)
        .fluid(160)
        .height(grid::aspect_ratio(160, 88));
    for e in entries {
        cards = cards.push(preset_card(model, e, editing, mode));
    }

    let search = tip(
        text_input("Search presets…", &model.search)
            .id(crate::search_id())
            .on_input(Message::SearchChanged)
            .size(13)
            .style(theme::text_input_style)
            .width(Length::Fixed(190.0)),
        help::SEARCH,
    );

    let soft = |icon: Element<'a>, label: &'a str, msg: Message, help: &'a str| -> Element<'a> {
        tip(
            button(
                row![icon, text(label).size(13)]
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .style(theme::button_soft)
            .padding([8, 12])
            .on_press(msg),
            help,
        )
    };
    let mut actions = row![icons::search(15.0, t.text_dim), search]
        .spacing(8)
        .align_y(Alignment::Center);
    if model.saving {
        actions = actions.push(
            text_input("Preset name", &model.save_name)
                .on_input(Message::SaveNameChanged)
                .on_submit(Message::SavePreset)
                .size(13)
                .style(theme::text_input_style)
                .width(Length::Fixed(170.0)),
        );
        actions = actions.push(
            button(text("Save").size(13))
                .style(theme::button_primary)
                .padding([8, 12])
                .on_press(Message::SavePreset),
        );
        actions = actions.push(
            button(text("Cancel").size(13))
                .style(theme::button_ghost)
                .padding([8, 12])
                .on_press(Message::CancelSave),
        );
    } else if editing {
        if host.capabilities().standalone {
            actions = actions.push(soft(
                icons::download(14.0, t.text),
                "Import…",
                Message::PickFile(crate::FileRequest::ImportPreset),
                help::IMPORT_PRESET,
            ));
        }
        actions = actions.push(
            button(text("Done").size(13))
                .style(theme::button_primary)
                .padding([8, 14])
                .on_press(Message::TogglePresetEdit),
        );
    } else {
        // Settings changed since a preset of the user's own was applied:
        // offer to keep them in it.
        if model.current_preset.is_none()
            && let Some(name) = &model.last_user_preset
        {
            actions = actions.push(tip(
                button(text(format!("Update “{}”", short(name, 14))).size(13))
                    .style(theme::button_soft)
                    .padding([8, 12])
                    .on_press(Message::UpdatePreset(name.clone())),
                help::UPDATE_PRESET,
            ));
        }
        actions = actions.push(soft(
            icons::plus(14.0, t.text),
            "Save as…",
            Message::StartSave,
            help::SAVE_PRESET,
        ));
        actions = actions.push(soft(
            icons::pencil(14.0, t.text),
            "Edit",
            Message::TogglePresetEdit,
            help::EDIT_PRESETS,
        ));
    }

    let body: Element<'a> = if found {
        cards.into()
    } else {
        container(dim(help::NO_PRESETS_FOUND, 13, mode))
            .padding([18, 4])
            .into()
    };
    section_with("Presets", actions, mode, body)
}

/// One preset: click to apply. In edit mode the user's own presets show
/// their rename, export and delete buttons.
fn preset_card<'a>(model: &'a Model, e: &'a PresetEntry, editing: bool, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let name = e.preset.name.clone();
    let selected = model.current_preset.as_deref() == Some(name.as_str());
    let fav = model.favorites.contains(&name);
    let stripe = category_color(&name);
    let icon_button = |icon: Element<'a>, msg: Message, help: &'a str| -> Element<'a> {
        tip(
            button(icon)
                .style(theme::button_ghost)
                .padding(3)
                .on_press(msg),
            help,
        )
    };

    let asking = model.confirm_delete_preset.as_deref() == Some(name.as_str());
    let renaming = model
        .renaming
        .as_ref()
        .filter(|(old, _)| *old == name)
        .map(|(_, new)| new);

    let content: Element<'a> = if asking {
        column![
            text(format!("Delete “{}”?", short(&name, 14))).size(13),
            row![
                button(text("Delete").size(12))
                    .style(theme::button_danger)
                    .padding([4, 10])
                    .on_press(Message::DeletePreset(name.clone())),
                button(text("Keep").size(12))
                    .style(theme::button_soft)
                    .padding([4, 10])
                    .on_press(Message::CancelDeletePreset),
            ]
            .spacing(6),
        ]
        .spacing(6)
        .width(Length::Fill)
        .into()
    } else if let Some(new) = renaming {
        column![
            text_input("Name", new)
                .id(crate::rename_id())
                .on_input(Message::RenameChanged)
                .on_submit(Message::CommitRename)
                .size(13)
                .padding(4)
                .style(theme::text_input_style),
            dim("Enter to save, Esc to cancel", 11, mode),
        ]
        .spacing(4)
        .width(Length::Fill)
        .into()
    } else {
        let mut top = row![
            text(e.preset.name.as_str())
                .size(14)
                .wrapping(iced_core::text::Wrapping::None),
            iced_widget::space::horizontal(),
        ]
        .align_y(Alignment::Center);
        if editing {
            if e.is_factory() {
                top = top.push(dim("built in", 10, mode));
            } else {
                top = top.push(icon_button(
                    icons::pencil(13.0, t.text_dim),
                    Message::StartRenamePreset(name.clone()),
                    help::RENAME_PRESET,
                ));
                top = top.push(icon_button(
                    icons::upload(13.0, t.text_dim),
                    Message::PickFile(crate::FileRequest::ExportPreset(name.clone())),
                    help::EXPORT_PRESET,
                ));
                top = top.push(icon_button(
                    icons::trash(13.0, theme::DANGER),
                    Message::AskDeletePreset(name.clone()),
                    help::DELETE_PRESET,
                ));
            }
        } else {
            top = top.push(icon_button(
                icons::star(13.0, if fav { theme::AMBER } else { t.text_dim }),
                Message::ToggleFavorite(name.clone()),
                help::FAVOURITE,
            ));
        }
        column![top, dim(short(&e.preset.description, 64), 11, mode)]
            .spacing(2)
            .width(Length::Fill)
            .into()
    };

    let body = row![
        container(iced_widget::space::vertical())
            .width(Length::Fixed(3.0))
            .height(Length::Fill)
            .style(move |_t| container::Style {
                background: Some(stripe.into()),
                border: theme::radius(2.0),
                ..container::Style::default()
            }),
        content,
    ]
    .spacing(8)
    .height(Length::Fixed(72.0));
    let busy = asking || renaming.is_some();
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
        // While a card asks a question, clicking it must not apply the preset.
        .on_press_maybe((!busy).then_some(Message::ApplyPreset(name)));
    if busy {
        card.into()
    } else {
        tip(card, e.preset.description.as_str())
    }
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
    .spacing(28)
    .wrap();
    section_accent(
        "Quick tweaks",
        theme::ACCENT,
        mode,
        container(knobs).center_x(Length::Fill),
    )
}

#[cfg(feature = "ai")]
fn ai_card<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    use crate::widgets::knob::{LABEL_WIDTH, param_slider_as, param_toggle_as};
    use std::sync::atomic::Ordering::Relaxed;
    use vc_core::ai::AiState;

    let t = theme::tokens(mode);
    let p = host.params();
    let status = host.ai_status();
    let state = status.as_ref().map(|s| s.state());
    let current = p.ai_voice();
    let current_name = model
        .voices
        .iter()
        .find(|v| v.path.to_string_lossy() == current)
        .map(|v| v.name.clone());
    let label = |s: &'a str| {
        container(text(s).size(14).color(t.text_dim)).width(Length::Fixed(LABEL_WIDTH))
    };

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
                "(file is missing)".into()
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
    .width(Length::Fixed(240.0));

    let ai_ptr = p.ai_enabled.as_ptr();
    let enable = iced_widget::toggler(p.ai_enabled.value())
        .on_toggle_maybe((!current.is_empty()).then_some(move |b| Message::ParamBool(ai_ptr, b)))
        .style(theme::toggler_style);

    let mut main = column![
        row![
            label("Voice"),
            tip(picker, help::VOICE_PICKER),
            tip(
                button(icons::refresh(13.0, t.text_dim))
                    .style(theme::button_ghost)
                    .padding(5)
                    .on_press(Message::RefreshLibrary),
                help::RESCAN
            )
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        tip(
            row![label("Use this voice"), enable]
                .spacing(12)
                .align_y(Alignment::Center),
            help::param("AI Voice")
        ),
    ]
    .spacing(8);

    // Multi-speaker models: pick the speaker (instant, no reload).
    if let Some(v) = model
        .voices
        .iter()
        .find(|v| v.path.to_string_lossy() == current && v.speakers > 1)
    {
        let labels: Vec<String> = (0..v.speakers).map(|i| v.speaker_label(i)).collect();
        let cur = (p.ai_speaker.value().max(0) as u32).min(v.speakers - 1);
        let ptr = p.ai_speaker.as_ptr();
        let labels2 = labels.clone();
        let speaker =
            iced_widget::pick_list(labels, Some(v.speaker_label(cur)), move |label: String| {
                let i = labels2.iter().position(|l| *l == label).unwrap_or(0) as f32;
                Message::ParamJump(ptr, i / 15.0)
            })
            .style(theme::pick_list_style)
            .text_size(13)
            .width(Length::Fixed(240.0));
        main = main.push(
            row![label("Speaker"), tip(speaker, help::param("AI Speaker"))]
                .spacing(12)
                .align_y(Alignment::Center),
        );
    }
    main = main.push(param_slider_as(&p.ai_pitch, "Pitch", mode, 170.0));
    main = main.push(super::enum_picker_as(&p.ai_speed, "Delay", mode, 240.0));
    main = main.push(param_toggle_as(&p.ai_block_noises, "Block coughs", mode));
    if p.ai_block_noises.value() {
        let ready = status
            .as_ref()
            .is_some_and(|s| s.blocker_ready.load(Relaxed));
        if ready {
            let n = status
                .as_ref()
                .map(|s| s.blocked.load(Relaxed))
                .unwrap_or(0);
            if n > 0 {
                main = main.push(dim(
                    format!("{n} coughs, burps or sneezes kept out so far."),
                    12,
                    mode,
                ));
            }
        } else if !model.cache.blocker_model_present {
            main = main.push(
                row![
                    dim("This needs a small extra download (15 MB).", 12, mode),
                    button(text("Download").size(12))
                        .style(theme::button_soft)
                        .padding([3, 10])
                        .on_press_maybe(
                            model
                                .import
                                .is_none()
                                .then_some(Message::DownloadBaseModels)
                        ),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
    }
    // Room noise goes through the model like speech does.
    if p.ai_enabled.value() && !p.denoise.value() {
        let ptr = p.denoise.as_ptr();
        main = main.push(
            row![
                dim(
                    "Noise removal is off: keyboard and fan noise will be turned into voice too.",
                    12,
                    mode
                ),
                button(text("Turn on").size(12))
                    .style(theme::button_soft)
                    .padding([3, 10])
                    .on_press(Message::ParamBool(ptr, true)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    // ---- what is going on, in a sentence ------------------------------------
    let active = p.ai_enabled.value();
    let mut problem: Option<Element<'a>> = None;
    let (status_text, color): (String, iced_core::Color) = match (&status, state) {
        (None, _) | (_, None) => ("AI voices are not available here.".into(), t.text_dim),
        (Some(_), Some(AiState::Off)) => {
            if model.voices.is_empty() {
                ("You have no AI voices yet.".into(), t.text_dim)
            } else {
                ("Pick a voice above to use it.".into(), t.text_dim)
            }
        }
        (Some(_), Some(AiState::Loading)) => (
            "Loading the voice… your own voice goes through meanwhile.".into(),
            theme::ink(theme::INDIGO, mode),
        ),
        (Some(st), Some(AiState::Error)) => {
            problem = Some(ai_problem(model, st.message(), mode));
            (String::new(), t.text_dim)
        }
        (Some(st), Some(AiState::Ready)) => {
            let load = st.load_factor();
            if !active {
                ("Ready. Switch on “Use this voice”.".into(), t.text_dim)
            } else if st.warming.load(Relaxed) {
                (
                    "Warming up: your own voice goes through until the AI voice has caught up."
                        .into(),
                    theme::ink(theme::AMBER, mode),
                )
            } else if load > 0.9 {
                (
                    "This computer can barely keep up, so the voice may stutter. Pick a slower Delay above.".into(),
                    theme::ink(theme::WARN, mode),
                )
            } else {
                (
                    format!(
                        "Working. {} · using {:.0}% of what this computer can do.",
                        capitalize(&super::topbar::delay_text(host)),
                        load * 100.0
                    ),
                    t.text_dim,
                )
            }
        }
    };

    // ---- the settings most people never touch ---------------------------------
    let mut advanced = column![].spacing(8);
    if model.ai_advanced {
        advanced = advanced.push(param_slider_as(&p.ai_breath, "Breathiness", mode, 170.0));
        advanced = advanced.push(param_slider_as(
            &p.ai_volume_follow,
            "Follow my loudness",
            mode,
            170.0,
        ));
        advanced = advanced.push(param_toggle_as(&p.ai_solo, "Only the AI voice", mode));
        let has_index = status.as_ref().is_some_and(|s| s.has_index.load(Relaxed))
            || model
                .voices
                .iter()
                .any(|v| v.path.to_string_lossy() == current && v.has_index);
        if has_index {
            advanced = advanced.push(param_toggle_as(&p.ai_index_enabled, "Closer match", mode));
            if p.ai_index_enabled.value() {
                advanced = advanced.push(param_slider_as(
                    &p.ai_index_rate,
                    "Match strength",
                    mode,
                    170.0,
                ));
                advanced = advanced.push(param_slider_as(
                    &p.ai_protect,
                    "Keep consonants",
                    mode,
                    170.0,
                ));
            }
        }
        // Where the AI runs (options snapshotted in the cache).
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
        .width(Length::Fixed(240.0));
        advanced = advanced.push(
            row![label("Runs on"), tip(compute_pick, help::COMPUTE)]
                .spacing(12)
                .align_y(Alignment::Center),
        );
        if let Some(n) = &model.cache.gpu_note {
            advanced = advanced.push(dim(n.clone(), 12, mode));
        }
        if let Some(st) = &status
            && active
            && state == Some(AiState::Ready)
        {
            let mut facts = vec![format!(
                "{:.0} ms to convert each piece on {}",
                st.infer_ms.load(Relaxed),
                vc_core::ai::rvc::backend_name()
            )];
            let voice_hz = st.f0_hz.load(Relaxed);
            if voice_hz > 0.0 {
                facts.push(format!("voice pitch {voice_hz:.0} Hz"));
            }
            let my_hz = st.speaker_f0_hz.load(Relaxed);
            if my_hz > 0.0 {
                facts.push(format!("your pitch {my_hz:.0} Hz"));
            }
            let drops = st.dropouts.load(Relaxed);
            if drops > 0 {
                facts.push(format!("{drops} gaps"));
            }
            advanced = advanced.push(dim(facts.join(" · "), 12, mode));
        }
    }
    let more = button(
        text(if model.ai_advanced {
            "Fewer settings"
        } else {
            "More settings"
        })
        .size(13),
    )
    .style(theme::button_ghost)
    .padding([4, 8])
    .on_press(Message::ToggleAiAdvanced);

    let state_badge = match state {
        Some(AiState::Ready) if active => badge("ON", theme::INDIGO),
        Some(AiState::Ready) => badge("READY", t.text_dim),
        Some(AiState::Loading) => badge("LOADING", theme::AMBER),
        Some(AiState::Error) => badge("PROBLEM", theme::DANGER),
        _ => badge("OFF", t.text_dim),
    };
    let manage = tip(
        button(
            text(if model.voices.is_empty() {
                "Add a voice"
            } else {
                "Manage voices"
            })
            .size(13),
        )
        .style(if model.voices.is_empty() {
            theme::button_indigo
        } else {
            theme::button_soft
        })
        .padding([6, 10])
        .on_press(Message::Go(crate::Page::Voices)),
        help::MANAGE_VOICES,
    );

    // Nothing to set up until there is a voice: say what this is for instead
    // of showing a column of switches that do nothing.
    if model.voices.is_empty() && current.is_empty() && problem.is_none() {
        let intro = column![
            text("Sound like a specific person or character.")
                .size(14)
                .color(t.text),
            dim(
                "An AI voice replaces your voice with another one while you talk. It runs on this computer; nothing you say is sent anywhere. Add a voice to start.",
                13,
                mode
            ),
        ]
        .spacing(4);
        return container(
            column![
                row![
                    text("AI voice").size(16).color(t.text),
                    iced_widget::space::horizontal(),
                    manage
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                intro
            ]
            .spacing(10),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::panel_accent(theme::INDIGO))
        .into();
    }
    let mut body = column![main].spacing(10);
    if let Some(problem) = problem {
        body = body.push(problem);
    } else if !status_text.is_empty() {
        body = body.push(
            text(status_text)
                .size(13)
                .color(color)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        );
    }
    body = body.push(more);
    if model.ai_advanced {
        body = body.push(advanced);
    }
    container(
        column![
            row![
                text("AI voice").size(16).color(t.text),
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

#[cfg(feature = "ai")]
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The AI could not start: say what to do about it, with the button that
/// does it, and keep the technical message for whoever wants it.
#[cfg(feature = "ai")]
fn ai_problem<'a>(model: &'a Model, raw: String, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let lower = raw.to_lowercase();
    let (headline, action): (&str, Option<(&str, Message)>) = if !model.cache.base_models_present {
        (
            "The AI needs its base models first (about 740 MB, downloaded once).",
            Some(("Download", Message::DownloadBaseModels)),
        )
    } else if lower.contains("onnx runtime") || lower.contains("onnxruntime") {
        (
            "The AI engine is missing or could not be loaded.",
            Some(("Open Settings", Message::Go(crate::Page::Settings))),
        )
    } else if lower.contains("no such file") || lower.contains("not found") {
        (
            "The voice's file is gone. Pick another voice, or add it again.",
            Some(("Manage voices", Message::Go(crate::Page::Voices))),
        )
    } else {
        (
            "The voice could not be loaded. It is tried again every half minute; picking another voice tries at once.",
            Some(("See the log", Message::Go(crate::Page::Settings))),
        )
    };
    let mut line = row![
        icons::alert(18.0, theme::DANGER),
        text(headline)
            .size(13)
            .color(t.text)
            .width(Length::Fill)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some((label, msg)) = action {
        let busy = matches!(msg, Message::DownloadBaseModels) && model.import.is_some();
        line = line.push(
            button(text(if busy { "Downloading…" } else { label }).size(13))
                .style(theme::button_primary)
                .padding([6, 12])
                .on_press_maybe((!busy).then_some(msg)),
        );
    }
    container(column![line, dim(format!("Details: {raw}"), 11, mode)].spacing(6))
        .padding(12)
        .width(Length::Fill)
        .style(theme::panel_accent(theme::DANGER))
        .into()
}
