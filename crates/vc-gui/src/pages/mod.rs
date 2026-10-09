//! The five pages plus the always-visible top bar.

pub mod effects;
pub mod home;
pub mod mixer;
pub mod settings;
pub mod topbar;
pub mod voices;

use crate::theme::{self, Mode};
use crate::{Element, Message};
use iced_core::{Alignment, Length};
use iced_widget::{column, container, pick_list, row, text};
use nice_plug::prelude::*;

/// A rounded panel with a title line.
pub fn section<'a>(title: &'a str, mode: Mode, body: impl Into<Element<'a>>) -> Element<'a> {
    let t = theme::tokens(mode);
    container(column![text(title).size(16).color(t.text), body.into()].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(theme::panel)
        .into()
}

/// A panel whose title line has something on the right (badge, button).
pub fn section_with<'a>(
    title: &'a str,
    right: impl Into<Element<'a>>,
    mode: Mode,
    body: impl Into<Element<'a>>,
) -> Element<'a> {
    let t = theme::tokens(mode);
    container(
        column![
            row![
                text(title).size(16).color(t.text),
                iced_widget::space::horizontal(),
                right.into()
            ]
            .align_y(Alignment::Center),
            body.into()
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::panel)
    .into()
}

pub fn dim<'a>(
    s: impl iced_core::text::IntoFragment<'a>,
    size: u32,
    mode: Mode,
) -> iced_widget::Text<'a, iced_core::Theme, iced_renderer::Renderer> {
    text(s)
        .size(size)
        .color(theme::tokens(mode).text_dim)
        .wrapping(iced_core::text::Wrapping::WordOrGlyph)
}

pub fn badge<'a>(label: &'a str, color: iced_core::Color) -> Element<'a> {
    container(text(label).size(11).color(color))
        .padding([2, 8])
        .style(move |_theme| container::Style {
            background: Some(iced_core::Color { a: 0.16, ..color }.into()),
            border: theme::radius(999.0),
            ..container::Style::default()
        })
        .into()
}

/// A labelled dropdown for an enum parameter.
pub fn enum_picker<'a, T>(param: &'a EnumParam<T>, mode: Mode, width: f32) -> Element<'a>
where
    T: Enum + PartialEq + Clone + 'static,
{
    let t = theme::tokens(mode);
    let variants: &'static [&'static str] = T::variants();
    let idx = param.value().to_index();
    let ptr = param.as_ptr();
    let n = variants.len().max(2) as f32 - 1.0;
    let picker = pick_list(
        variants,
        variants.get(idx).copied(),
        move |label: &'static str| {
            let i = variants.iter().position(|v| *v == label).unwrap_or(0);
            Message::ParamJump(ptr, i as f32 / n)
        },
    )
    .style(theme::pick_list_style)
    .width(Length::Fixed(width));
    let row = row![
        container(text(param.name()).size(14).color(t.text_dim)).width(Length::Fixed(130.0)),
        picker
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    crate::tip(row, crate::help::param(param.name()))
}

/// A panel with a coloured edge and title.
pub fn section_accent<'a>(
    title: &'a str,
    accent: iced_core::Color,
    mode: Mode,
    body: impl Into<Element<'a>>,
) -> Element<'a> {
    let t = theme::tokens(mode);
    container(column![text(title).size(16).color(t.text), body.into()].spacing(12))
        .padding(16)
        .width(Length::Fill)
        .style(theme::panel_accent(accent))
        .into()
}

/// Accent panel whose title line has something on the right.
pub fn section_accent_with<'a>(
    title: &'a str,
    accent: iced_core::Color,
    right: impl Into<Element<'a>>,
    mode: Mode,
    body: impl Into<Element<'a>>,
) -> Element<'a> {
    let t = theme::tokens(mode);
    container(
        column![
            row![
                text(title).size(16).color(t.text),
                iced_widget::space::horizontal(),
                right.into()
            ]
            .align_y(Alignment::Center),
            body.into()
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::panel_accent(accent))
    .into()
}

/// A key/value line for status panels.
pub fn kv<'a>(key: &'a str, value: impl Into<String>, mode: Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    row![
        container(text(key).size(13).color(t.text_dim)).width(Length::Fixed(130.0)),
        text(value.into()).size(13)
    ]
    .spacing(12)
    .into()
}
