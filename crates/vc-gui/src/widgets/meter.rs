//! Smoothed level meters drawn with the canvas.

use crate::theme;
use iced_core::{Color, Length, Point, Rectangle, Size, Theme, mouse};
use iced_widget::canvas as cv;
use iced_widget::canvas::{self, Frame, Geometry, Path};

#[derive(Default, Clone)]
pub struct Levels {
    pub input: f32,
    pub output: f32,
    pub in_hold: f32,
    pub out_hold: f32,
    pub gate_open: bool,
    hold_age: f32,
}

impl Levels {
    pub fn push(&mut self, input: f32, output: f32, gate_open: bool, dt: f32) {
        let fall = (-dt * 4.0).exp();
        self.input = input.max(self.input * fall);
        self.output = output.max(self.output * fall);
        self.hold_age += dt;
        if input >= self.in_hold || self.hold_age > 1.0 {
            self.in_hold = input;
        }
        if output >= self.out_hold || self.hold_age > 1.0 {
            self.out_hold = output;
        }
        if self.hold_age > 1.0 {
            self.hold_age = 0.0;
        }
        self.gate_open = gate_open;
    }
}

fn db_norm(level: f32) -> f32 {
    if level <= 1e-5 {
        return 0.0;
    }
    ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub struct Meter {
    pub level: f32,
    pub hold: f32,
    pub muted: bool,
    pub mode: theme::Mode,
}

impl<Message> canvas::Program<Message, Theme, iced_renderer::Renderer> for Meter {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced_renderer::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let t = theme::tokens(self.mode);
        let mut frame = Frame::new(renderer, bounds.size());
        let h = bounds.height;
        let r = h / 2.0;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), r.into()),
            t.meter_bg,
        );
        let w = bounds.width * db_norm(self.level);
        if w > 1.0 {
            let color = if self.level >= 0.99 {
                theme::DANGER
            } else if self.muted {
                t.text_dim
            } else {
                theme::ACCENT
            };
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, Size::new(w, h), r.into()),
                color,
            );
        }
        let hx = bounds.width * db_norm(self.hold);
        if self.hold > 1e-4 {
            frame.fill(
                &Path::rectangle(Point::new(hx - 1.0, 0.0), Size::new(2.0, h)),
                Color::WHITE,
            );
        }
        let tick = bounds.width * 0.8;
        frame.fill(
            &Path::rectangle(Point::new(tick, 0.0), Size::new(1.0, h)),
            Color {
                a: 0.25,
                ..Color::WHITE
            },
        );
        vec![frame.into_geometry()]
    }
}

pub fn meter<'a, Message: 'a>(
    level: f32,
    hold: f32,
    muted: bool,
    mode: theme::Mode,
    width: Length,
) -> iced_core::Element<'a, Message, Theme, iced_renderer::Renderer> {
    cv::Canvas::new(Meter {
        level,
        hold,
        muted,
        mode,
    })
    .width(width)
    .height(Length::Fixed(12.0))
    .into()
}
