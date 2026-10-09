//! A horizontal slider drawn on the canvas, so it behaves exactly like the
//! knobs: drag, mouse wheel, double-click or right-click to reset. (iced's
//! own slider captures its presses, which kept a wrapping `mouse_area` from
//! ever seeing a double-click.)

use crate::theme;
use crate::{Element, Message};
use iced_core::{Color, Length, Point, Rectangle, Size, Theme, mouse};
use iced_widget::Action;
use iced_widget::canvas::{self, Frame, Geometry, Path};
use nice_plug::prelude::*;
use std::time::Instant;

pub struct Slider {
    ptr: ParamPtr,
    value: f32,
    mode: theme::Mode,
    accent: Color,
}

#[derive(Default)]
pub struct SliderState {
    dragging: bool,
    last_click: Option<Instant>,
}

const HANDLE_R: f32 = 8.0;

impl Slider {
    fn value_at(&self, x: f32, bounds: Rectangle) -> f32 {
        ((x - bounds.x - HANDLE_R) / (bounds.width - 2.0 * HANDLE_R)).clamp(0.0, 1.0)
    }
}

impl canvas::Program<Message, Theme, iced_renderer::Renderer> for Slider {
    type State = SliderState;

    fn update(
        &self,
        state: &mut SliderState,
        event: &iced_core::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        use iced_core::mouse::{Button, Event as M, ScrollDelta};
        let cursor = cursor.land();
        match event {
            iced_core::Event::Mouse(M::ButtonPressed(Button::Left)) => {
                let pos = cursor.position_over(bounds)?;
                let now = Instant::now();
                let double = state
                    .last_click
                    .is_some_and(|t| now.duration_since(t).as_millis() < 350);
                state.last_click = Some(now);
                if double {
                    state.dragging = false;
                    return Some(Action::publish(Message::ParamReset(self.ptr)).and_capture());
                }
                state.dragging = true;
                // Jump to the click, then follow the drag.
                Some(
                    Action::publish(Message::ParamPress(self.ptr, self.value_at(pos.x, bounds)))
                        .and_capture(),
                )
            }
            iced_core::Event::Mouse(M::ButtonPressed(Button::Right)) => {
                cursor.position_over(bounds)?;
                Some(Action::publish(Message::ParamReset(self.ptr)).and_capture())
            }
            iced_core::Event::Mouse(M::CursorMoved { .. }) => {
                if !state.dragging {
                    return None;
                }
                let x = cursor.position()?.x;
                Some(
                    Action::publish(Message::ParamSet(self.ptr, self.value_at(x, bounds)))
                        .and_capture(),
                )
            }
            iced_core::Event::Mouse(M::ButtonReleased(Button::Left))
            | iced_core::Event::Mouse(M::CursorLeft) => {
                if !state.dragging {
                    return None;
                }
                state.dragging = false;
                Some(Action::publish(Message::ParamEnd(self.ptr)).and_capture())
            }
            iced_core::Event::Mouse(M::WheelScrolled { delta }) => {
                cursor.position_over(bounds)?;
                let step = match delta {
                    ScrollDelta::Lines { y, .. } => *y,
                    ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                Some(
                    Action::publish(Message::ParamJump(
                        self.ptr,
                        (self.value + step * 0.02).clamp(0.0, 1.0),
                    ))
                    .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &SliderState,
        renderer: &iced_renderer::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let t = theme::tokens(self.mode);
        let mut frame = Frame::new(renderer, bounds.size());
        let cy = bounds.height / 2.0;
        let rail_h = 6.0;
        let x0 = HANDLE_R;
        let w = bounds.width - 2.0 * HANDLE_R;
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(x0, cy - rail_h / 2.0),
                Size::new(w, rail_h),
                (rail_h / 2.0).into(),
            ),
            t.raised,
        );
        let fill_w = w * self.value;
        if fill_w > 0.5 {
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(x0, cy - rail_h / 2.0),
                    Size::new(fill_w, rail_h),
                    (rail_h / 2.0).into(),
                ),
                self.accent,
            );
        }
        let hovered = state.dragging || cursor.is_over(bounds);
        let hx = x0 + fill_w;
        frame.fill(
            &Path::circle(Point::new(hx, cy), HANDLE_R),
            if hovered {
                Color::WHITE
            } else {
                Color::from_rgb8(230, 234, 240)
            },
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &SliderState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

/// The bare slider for a parameter (no label or readout).
pub fn slider<'a>(
    ptr: ParamPtr,
    value: f32,
    mode: theme::Mode,
    accent: Color,
    width: f32,
) -> Element<'a> {
    canvas::Canvas::new(Slider {
        ptr,
        value,
        mode,
        accent,
    })
    .width(Length::Fixed(width))
    .height(Length::Fixed(22.0))
    .into()
}
