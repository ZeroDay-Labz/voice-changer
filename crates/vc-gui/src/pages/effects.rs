//! Effects: pitch engine and the effect panels, all knobs.

use super::{dim, enum_picker, section, section_accent_with};
use crate::host::Host;
use crate::theme;
use crate::theme::Mode;
use crate::widgets::knob::knob;
use crate::{Element, Model};
use iced_core::Color;
use iced_core::Length;
use iced_widget::{column, container, row};
use nice_plug::prelude::*;

pub fn view<'a>(_model: &'a Model, host: &'a dyn Host) -> Element<'a> {
    let mode = host.theme_mode();
    let p = host.params();
    let engine = column![
        enum_picker(&p.pitch_engine, mode, 220.0),
        dim(
            "Natural (default) keeps your voice human for small to medium shifts. Fast / Balanced / Smooth are phase-vocoder modes: more robotic on big shifts, lower latency.",
            12,
            mode
        ),
    ]
    .spacing(8);
    column![
        section("Pitch engine", mode, engine),
        row![
            panel(
                "Voice",
                theme::ACCENT,
                mode,
                &[(&p.pitch, "Pitch"), (&p.formant, "Formant")]
            ),
            panel("Drive", theme::AMBER, mode, &[(&p.drive, "Drive")]),
            panel(
                "Robot",
                theme::CORAL,
                mode,
                &[(&p.ring_mix, "Amount"), (&p.ring_freq, "Frequency")]
            ),
        ]
        .spacing(16)
        .wrap(),
        row![
            panel(
                "Echo",
                theme::SKY,
                mode,
                &[
                    (&p.echo_mix, "Mix"),
                    (&p.echo_time, "Time"),
                    (&p.echo_feedback, "Feedback")
                ]
            ),
            panel(
                "Reverb",
                theme::INDIGO,
                mode,
                &[
                    (&p.reverb_mix, "Mix"),
                    (&p.reverb_size, "Size"),
                    (&p.reverb_damp, "Damping")
                ]
            ),
            panel(
                "Tone",
                theme::VIOLET,
                mode,
                &[(&p.eq_low, "Low"), (&p.eq_mid, "Mid"), (&p.eq_high, "High")]
            ),
        ]
        .spacing(16)
        .wrap(),
    ]
    .spacing(16)
    .into()
}

fn panel<'a>(
    title: &'a str,
    accent: Color,
    mode: Mode,
    knobs: &[(&'a FloatParam, &str)],
) -> Element<'a> {
    let t = theme::tokens(mode);
    let mut r = row![].spacing(18);
    for (param, label) in knobs {
        r = r.push(knob(*param, Some(label), mode, 84.0, accent));
    }
    let ptrs: Vec<nice_plug::prelude::ParamPtr> = knobs.iter().map(|(p, _)| p.as_ptr()).collect();
    let reset = crate::tip(
        iced_widget::button(iced_widget::text("Reset").size(12).color(t.text_dim))
            .style(theme::button_ghost)
            .padding([3, 8])
            .on_press(crate::Message::ResetMany(ptrs)),
        crate::help::RESET_PANEL,
    );
    container(section_accent_with(
        title,
        accent,
        reset,
        mode,
        container(r).center_x(Length::Fill),
    ))
    .width(Length::Fixed(60.0 + 102.0 * knobs.len() as f32))
    .into()
}
