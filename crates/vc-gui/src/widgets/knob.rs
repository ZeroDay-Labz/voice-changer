//! A rotary knob: drag vertically, scroll to nudge, double- or right-click
//! to reset. Emits the same begin/set/end gestures as the sliders.

use crate::theme;
use crate::{Element, Message};
use iced_core::{Color, Length, Point, Rectangle, Theme, mouse};
use iced_widget::Action;
use iced_widget::canvas::{self, Frame, Geometry, Path, Stroke, stroke};
use iced_widget::{column, container, text};
use nice_plug::prelude::*;
use std::time::Instant;

pub struct Knob {
    ptr: ParamPtr,
    value: f32,
    label: String,
    readout: String,
    mode: theme::Mode,
    accent: Color,
}

#[derive(Default)]
pub struct KnobState {
    dragging: Option<(f32, f32)>, // (start y, start value)
    last_click: Option<Instant>,
}

impl canvas::Program<Message, Theme, iced_renderer::Renderer> for Knob {
    type State = KnobState;

    fn update(
        &self,
        state: &mut KnobState,
        event: &iced_core::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        use iced_core::mouse::{Button, Event as M, ScrollDelta};
        match event {
            iced_core::Event::Mouse(M::ButtonPressed(Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                let now = Instant::now();
                let double = state
                    .last_click
                    .is_some_and(|t| now.duration_since(t).as_millis() < 350);
                state.last_click = Some(now);
                if double {
                    state.dragging = None;
                    return Some(Action::publish(Message::ParamReset(self.ptr)).and_capture());
                }
                state.dragging = Some((pos.y, self.value));
                Some(Action::publish(Message::ParamBegin(self.ptr)).and_capture())
            }
            iced_core::Event::Mouse(M::ButtonPressed(Button::Right)) => {
                cursor.position_in(bounds)?;
                Some(Action::publish(Message::ParamReset(self.ptr)).and_capture())
            }
            iced_core::Event::Mouse(M::CursorMoved { .. }) => {
                let (start_y, start_v) = state.dragging?;
                let y = cursor.position()?.y;
                let v = (start_v + (start_y - y) / 120.0).clamp(0.0, 1.0);
                Some(Action::publish(Message::ParamSet(self.ptr, v)).and_capture())
            }
            iced_core::Event::Mouse(M::ButtonReleased(Button::Left))
            | iced_core::Event::Mouse(M::CursorLeft) => {
                state.dragging.take()?;
                Some(Action::publish(Message::ParamEnd(self.ptr)).and_capture())
            }
            iced_core::Event::Mouse(M::WheelScrolled { delta }) => {
                cursor.position_in(bounds)?;
                let step = match delta {
                    ScrollDelta::Lines { y, .. } => *y,
                    ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                let v = (self.value + step * 0.02).clamp(0.0, 1.0);
                Some(Action::publish(Message::ParamJump(self.ptr, v)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &KnobState,
        renderer: &iced_renderer::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let t = theme::tokens(self.mode);
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = bounds.width.min(bounds.height) / 2.0 - 4.0;
        let start = 0.75 * std::f32::consts::PI;
        let sweep = 1.5 * std::f32::consts::PI;
        let track = Path::new(|b| {
            b.arc(canvas::path::Arc {
                center,
                radius,
                start_angle: iced_core::Radians(start),
                end_angle: iced_core::Radians(start + sweep),
            })
        });
        frame.stroke(
            &track,
            Stroke {
                style: stroke::Style::Solid(t.raised_hover),
                width: 6.0,
                line_cap: stroke::LineCap::Round,
                ..Stroke::default()
            },
        );
        if self.value > 0.001 {
            let arc = Path::new(|b| {
                b.arc(canvas::path::Arc {
                    center,
                    radius,
                    start_angle: iced_core::Radians(start),
                    end_angle: iced_core::Radians(start + sweep * self.value),
                })
            });
            frame.stroke(
                &arc,
                Stroke {
                    style: stroke::Style::Solid(self.accent),
                    width: 6.0,
                    line_cap: stroke::LineCap::Round,
                    ..Stroke::default()
                },
            );
        }
        let hovered = cursor.is_over(bounds) || state.dragging.is_some();
        frame.fill(
            &Path::circle(center, radius - 8.0),
            if hovered { t.raised_hover } else { t.raised },
        );
        let angle = start + sweep * self.value;
        let (s, c) = angle.sin_cos();
        let p1 = Point::new(
            center.x + c * (radius - 20.0),
            center.y + s * (radius - 20.0),
        );
        let p2 = Point::new(
            center.x + c * (radius - 10.0),
            center.y + s * (radius - 10.0),
        );
        frame.stroke(
            &Path::line(p1, p2),
            Stroke {
                style: stroke::Style::Solid(Color::WHITE),
                width: 3.0,
                line_cap: stroke::LineCap::Round,
                ..Stroke::default()
            },
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &KnobState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

/// A labelled knob for any continuous parameter, in the accent colour of
/// its panel. Hovering shows the parameter's help text.
pub fn knob<'a, P: Param>(
    param: &'a P,
    label: Option<&str>,
    mode: theme::Mode,
    size: f32,
    accent: Color,
) -> Element<'a> {
    let value = param.unmodulated_normalized_value();
    let k = Knob {
        ptr: param.as_ptr(),
        value,
        label: label
            .map(str::to_string)
            .unwrap_or_else(|| param.name().to_string()),
        readout: param.normalized_value_to_string(value, true),
        mode,
        accent,
    };
    let t = theme::tokens(mode);
    let label = k.label.clone();
    let readout = k.readout.clone();
    let body = column![
        canvas::Canvas::new(k)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
        text(label).size(13).color(t.text_dim),
        text(readout).size(13),
    ]
    .spacing(2)
    .align_x(iced_core::Alignment::Center);
    crate::tip(body, crate::help::param(param.name()))
}

/// Mouse-wheel nudging for sliders: one notch = 2% of the range.
fn wheel_step(ptr: ParamPtr, value: f32) -> impl Fn(iced_core::mouse::ScrollDelta) -> Message {
    move |delta| {
        let step = match delta {
            iced_core::mouse::ScrollDelta::Lines { y, .. } => y,
            iced_core::mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
        };
        Message::ParamJump(ptr, (value + step * 0.02).clamp(0.0, 1.0))
    }
}

fn slider_core<'a>(ptr: ParamPtr, value: f32, width: f32) -> Element<'a> {
    let slider = iced_widget::slider(0.0..=1.0f32, value, move |v| Message::ParamSet(ptr, v))
        .step(0.001f32)
        .on_release(Message::ParamEnd(ptr))
        .style(theme::slider_style)
        .width(Length::Fixed(width));
    iced_widget::mouse_area(slider)
        .on_right_press(Message::ParamReset(ptr))
        .on_double_click(Message::ParamReset(ptr))
        .on_scroll(wheel_step(ptr, value))
        .into()
}

/// Horizontal slider row for a parameter: label, slider, value.
pub fn param_slider<'a, P: Param>(param: &'a P, mode: theme::Mode, width: f32) -> Element<'a> {
    let t = theme::tokens(mode);
    let value = param.unmodulated_normalized_value();
    let readout = param.normalized_value_to_string(value, true);
    let row = iced_widget::row![
        container(text(param.name()).size(14).color(t.text_dim)).width(Length::Fixed(130.0)),
        slider_core(param.as_ptr(), value, width),
        container(text(readout).size(13)).width(Length::Fixed(80.0)),
    ]
    .spacing(12)
    .align_y(iced_core::Alignment::Center);
    crate::tip(row, crate::help::param(param.name()))
}

/// A labelled toggle for a bool parameter.
pub fn param_toggle<'a>(param: &'a BoolParam, mode: theme::Mode) -> Element<'a> {
    let t = theme::tokens(mode);
    let ptr = param.as_ptr();
    let row = iced_widget::row![
        container(text(param.name()).size(14).color(t.text_dim)).width(Length::Fixed(130.0)),
        iced_widget::toggler(param.value())
            .on_toggle(move |b| Message::ParamBool(ptr, b))
            .style(theme::toggler_style),
    ]
    .spacing(12)
    .align_y(iced_core::Alignment::Center);
    crate::tip(row, crate::help::param(param.name()))
}

/// Slider + value only (no label); used in the top bar next to the meters.
pub fn param_slider_compact<'a, P: Param>(
    param: &'a P,
    mode: theme::Mode,
    width: f32,
) -> Element<'a> {
    let t = theme::tokens(mode);
    let value = param.unmodulated_normalized_value();
    let readout = param.normalized_value_to_string(value, true);
    let row = iced_widget::row![
        slider_core(param.as_ptr(), value, width),
        container(text(readout).size(12).color(t.text_dim)).width(Length::Fixed(64.0))
    ]
    .spacing(8)
    .align_y(iced_core::Alignment::Center);
    crate::tip(row, crate::help::param(param.name()))
}
