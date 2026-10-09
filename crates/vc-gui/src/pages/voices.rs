#![cfg_attr(not(feature = "ai"), allow(unused_imports))]
//! Voices: import, list, delete AI voice models.

use super::{badge, dim, section_accent, section_with};
use crate::host::Host;
use crate::theme::{self, Mode};
use crate::widgets::icons;
use crate::{Element, Message, Model, tip};
use iced_core::{Alignment, Length};
use iced_widget::{button, column, container, row, text, text_input};

pub fn view<'a>(model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    #[cfg(feature = "ai")]
    {
        let mut col = column![].spacing(16);
        if !model.cache.base_models_present {
            col = col.push(base_models_banner(model, mode));
        }
        col = col.push(importer(model, mode));
        col = col.push(installed(model, host, mode));
        col.into()
    }
    #[cfg(not(feature = "ai"))]
    {
        let _ = model;
        section_accent(
            "AI voices",
            theme::INDIGO,
            mode,
            dim("This build has no AI support.", 13, mode),
        )
    }
}

#[cfg(feature = "ai")]
fn base_models_banner<'a>(model: &'a Model, mode: Mode) -> Element<'a> {
    let mut r = row![
        icons::download(18.0, theme::WARN),
        column![
            text("Base models are missing").size(15).color(theme::WARN),
            dim(
                "AI voices need ContentVec and RMVPE (about 740 MB, downloaded once).",
                12,
                mode
            )
        ]
        .spacing(2),
        iced_widget::space::horizontal(),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if model.import.is_none() {
        r = r.push(tip(
            button(text("Download base models").size(13))
                .style(theme::button_primary)
                .padding([8, 12])
                .on_press(Message::DownloadBaseModels),
            crate::help::DOWNLOAD_BASE,
        ));
    }
    container(r)
        .padding(14)
        .width(Length::Fill)
        .style(theme::panel)
        .into()
}

#[cfg(feature = "ai")]
fn importer<'a>(model: &'a Model, mode: Mode) -> Element<'a> {
    use vc_core::ai::Stage;
    let t = theme::tokens(mode);
    let busy = model.import.is_some();
    let can = !model.import_url.trim().is_empty() && !busy;
    let inputs = row![
        text_input(
            "https://huggingface.co/user/voice-model  (or a direct .pth / .onnx / .zip link)",
            &model.import_url
        )
        .on_input(Message::ImportUrlChanged)
        .on_submit_maybe(can.then_some(Message::StartImport))
        .size(13)
        .style(theme::text_input_style)
        .width(Length::Fill),
        text_input("Name (optional)", &model.import_name)
            .on_input(Message::ImportNameChanged)
            .size(13)
            .style(theme::text_input_style)
            .width(Length::Fixed(160.0)),
        button(
            row![
                icons::plus(14.0, iced_core::Color::from_rgb8(8, 30, 22)),
                text("Add").size(13)
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        )
        .style(theme::button_primary)
        .padding([8, 14])
        .on_press_maybe(can.then_some(Message::StartImport)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut body = column![
        dim("Paste a Hugging Face model page or a direct link. RVC v2 models work; .pth files are converted to ONNX automatically and a matching .index is picked up when present.", 12, mode),
        inputs,
    ]
    .spacing(10);

    if let Some(job) = &model.import {
        let stage = job.stage();
        let (line, frac): (String, Option<f32>) = match &stage {
            Stage::Downloading { file, done, total } => (
                format!(
                    "Downloading {file} · {:.0} MB{}",
                    *done as f32 / 1e6,
                    total
                        .map(|t| format!(" of {:.0}", t as f32 / 1e6))
                        .unwrap_or_default()
                ),
                total.map(|t| *done as f32 / t.max(1) as f32),
            ),
            Stage::Extracting => ("Unpacking…".into(), None),
            Stage::PreparingConverter => (
                "Setting up the converter (one-time, downloads ~300 MB of Python packages)…".into(),
                None,
            ),
            Stage::Converting => (
                "Converting the model to ONNX (about a minute)…".into(),
                None,
            ),
            Stage::Done(n) => (format!("Installed “{n}”"), Some(1.0)),
            Stage::Failed(e) => (format!("Failed: {e}"), None),
            Stage::Idle => (String::new(), None),
        };
        let failed = matches!(stage, Stage::Failed(_));
        let mut prog = column![
            row![
                text(line)
                    .size(13)
                    .color(if failed { theme::DANGER } else { t.text })
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                iced_widget::space::horizontal(),
                if failed {
                    button(text("Dismiss").size(12))
                        .style(theme::button_ghost)
                        .padding([4, 8])
                        .on_press(Message::DismissImport)
                } else {
                    button(text("Cancel").size(12))
                        .style(theme::button_ghost)
                        .padding([4, 8])
                        .on_press(Message::CancelImport)
                }
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(6);
        if let Some(f) = frac {
            prog = prog.push(
                iced_widget::progress_bar(0.0..=1.0, f)
                    .girth(6.0)
                    .style(theme::progress_style),
            );
        } else if !failed {
            prog = prog.push(
                iced_widget::progress_bar(0.0..=1.0, 0.0)
                    .girth(6.0)
                    .style(theme::progress_style),
            );
        }
        body = body.push(
            container(prog)
                .padding(10)
                .style(theme::raised)
                .width(Length::Fill),
        );
        // Keep the last log line visible for the curious.
        if let Ok(log) = job.log.lock()
            && let Some(last) = log.last()
        {
            body = body.push(dim(last.clone(), 11, mode));
        }
    }
    section_accent("Add a voice", theme::INDIGO, mode, body)
}

#[cfg(feature = "ai")]
fn installed<'a>(model: &'a Model, host: &'a dyn Host, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let current = host.params().ai_voice();
    let mut list = column![].spacing(8);
    if model.voices.is_empty() {
        list = list.push(dim("No voices yet. Add one above.", 13, mode));
    }
    for v in &model.voices {
        let path = v.path.to_string_lossy().to_string();
        let selected = path == current;
        let mut meta = row![dim(format!("{:.0} MB", v.size_mb), 12, mode)]
            .spacing(10)
            .align_y(Alignment::Center);
        if let Some(r) = v.sample_rate {
            meta = meta.push(dim(format!("{} kHz", r / 1000), 12, mode));
        }
        if v.has_index {
            meta = meta.push(tip(badge("index", theme::INDIGO), crate::help::INDEX_BADGE));
        }
        if v.speakers > 1 {
            meta = meta.push(badge(
                if v.speakers == 2 {
                    "2 speakers"
                } else {
                    "multi-speaker"
                },
                theme::VIOLET,
            ));
        }
        if let Some(src) = &v.source {
            meta = meta.push(dim(src.clone(), 12, mode));
        }
        let confirm = model.confirm_delete.as_deref() == Some(path.as_str());
        let actions: Element<'a> = if confirm {
            row![
                text("Delete this voice?").size(13).color(theme::DANGER),
                button(text("Delete").size(13))
                    .style(theme::button_danger)
                    .padding([6, 10])
                    .on_press(Message::ConfirmDeleteVoice),
                button(text("Keep").size(13))
                    .style(theme::button_ghost)
                    .padding([6, 10])
                    .on_press(Message::CancelDeleteVoice),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        } else {
            let use_btn = if selected {
                button(
                    row![
                        icons::check(14.0, iced_core::Color::WHITE),
                        text("In use").size(13)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .style(theme::button_indigo)
                .padding([6, 10])
            } else {
                button(text("Use").size(13))
                    .style(theme::button_soft)
                    .padding([6, 10])
                    .on_press(Message::UseVoice(path.clone()))
            };
            row![
                use_btn,
                tip(
                    button(icons::trash(14.0, t.text_dim))
                        .style(theme::button_ghost)
                        .padding(6)
                        .on_press(Message::AskDeleteVoice(path.clone())),
                    crate::help::DELETE_VOICE
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        };
        let card = row![
            icons::mask(22.0, if selected { theme::INDIGO } else { t.text_dim }),
            column![text(v.name.as_str()).size(15), meta]
                .spacing(4)
                .width(Length::Fill),
            actions,
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        let card: Element<'a> = column![card, speaker_row(model, v, mode)].spacing(8).into();
        list = list.push(
            container(card)
                .padding([10, 14])
                .width(Length::Fill)
                .style(move |th| {
                    if selected {
                        theme::panel_accent(theme::INDIGO)(th)
                    } else {
                        theme::raised(th)
                    }
                }),
        );
    }
    let right = row![
        dim(model.cache.voices_dir.clone(), 11, mode),
        tip(
            button(icons::refresh(14.0, t.text_dim))
                .style(theme::button_ghost)
                .padding(6)
                .on_press(Message::RefreshLibrary),
            crate::help::RESCAN,
        )
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    section_with("Installed voices", right, mode, list)
}

/// "Starts as" picker plus a name field per speaker, for multi-speaker voices.
#[cfg(feature = "ai")]
fn speaker_row<'a>(model: &'a Model, v: &'a vc_core::ai::VoiceInfo, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let path = v.path.to_string_lossy().to_string();
    let labels: Vec<String> = (0..v.speakers).map(|i| v.speaker_label(i)).collect();
    let labels2 = labels.clone();
    let path2 = path.clone();
    let starts = iced_widget::pick_list(
        labels,
        Some(v.speaker_label(v.default_speaker)),
        move |label: String| {
            Message::SetDefaultSpeaker(
                path2.clone(),
                labels2.iter().position(|l| *l == label).unwrap_or(0) as u32,
            )
        },
    )
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(150.0));
    let counts: Vec<String> = (1..=8).map(|n| n.to_string()).collect();
    let path3 = path.clone();
    let count = iced_widget::pick_list(counts, Some(v.speakers.to_string()), move |n: String| {
        Message::SetVoiceSpeakers(path3.clone(), n.parse().unwrap_or(1))
    })
    .style(theme::pick_list_style)
    .text_size(13)
    .width(Length::Fixed(64.0));
    let mut r = row![
        text("Speakers").size(13).color(t.text_dim),
        tip(count, crate::help::SPEAKER_COUNT)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if v.speakers < 2 {
        return container(r).padding(crate::pad(0.0, 0.0, 0.0, 36.0)).into();
    }
    r = r.push(text("Starts as").size(13).color(t.text_dim));
    r = r.push(tip(starts, crate::help::STARTS_AS));
    r = r.push(text("Names:").size(13).color(t.text_dim));
    for i in 0..v.speakers.min(8) {
        let key = (path.clone(), i);
        let value = model
            .speaker_edits
            .get(&key)
            .cloned()
            .unwrap_or_else(|| v.speaker_names.get(i as usize).cloned().unwrap_or_default());
        let (p1, p2) = (path.clone(), path.clone());
        r = r.push(tip(
            text_input(&format!("Speaker {}", i + 1), &value)
                .on_input(move |s| Message::SpeakerNameChanged(p1.clone(), i, s))
                .on_submit(Message::SaveSpeakerName(p2, i))
                .size(13)
                .style(theme::text_input_style)
                .width(Length::Fixed(110.0)),
            crate::help::SPEAKER_NAME,
        ));
    }
    container(r).padding(crate::pad(0.0, 0.0, 0.0, 36.0)).into()
}
