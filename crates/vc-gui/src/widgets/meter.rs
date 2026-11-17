//! Smoothed level meters drawn with the canvas.

use crate::theme;
use iced_core::{Length, Point, Rectangle, Size, Theme, mouse};
use iced_widget::canvas as cv;
use iced_widget::canvas::{self, Frame, Geometry, Path};

/// Lowest level the meters show, in dB.
const FLOOR_DB: f32 = -60.0;
/// The zone speech peaks should reach.
const GOOD_DB: (f32, f32) = (-18.0, -6.0);
/// How long a clip stays flagged, in seconds.
const CLIP_HOLD: f32 = 4.0;

#[derive(Default, Clone)]
pub struct Levels {
    pub input: f32,
    pub output: f32,
    pub in_hold: f32,
    pub out_hold: f32,
    pub gate_open: bool,
    hold_age: f32,
    /// Seconds since the input / output last hit full scale.
    in_clip_age: Option<f32>,
    out_clip_age: Option<f32>,
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
        let age = |slot: &mut Option<f32>, level: f32| {
            if level >= 0.99 {
                *slot = Some(0.0);
            } else if let Some(a) = slot {
                *a += dt;
                if *a > CLIP_HOLD {
                    *slot = None;
                }
            }
        };
        age(&mut self.in_clip_age, input);
        age(&mut self.out_clip_age, output);
    }

    /// The input hit full scale a moment ago.
    pub fn in_clipped(&self) -> bool {
        self.in_clip_age.is_some()
    }

    pub fn out_clipped(&self) -> bool {
        self.out_clip_age.is_some()
    }
}

fn db_to_norm(db: f32) -> f32 {
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

fn db_norm(level: f32) -> f32 {
    if level <= 1e-5 {
        return 0.0;
    }
    db_to_norm(20.0 * level.log10())
}

pub struct Meter {
    pub level: f32,
    pub hold: f32,
    /// Drawn grey: nothing of this is being used (gate closed, bypassed).
    pub muted: bool,
    pub clipped: bool,
    /// Draw the dB scale under the bar.
    pub scale: bool,
    pub mode: theme::Mode,
}

pub const BAR_HEIGHT: f32 = 12.0;
const SCALE_HEIGHT: f32 = 13.0;

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
        let h = BAR_HEIGHT;
        let r = h / 2.0;
        let width = bounds.width;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, Size::new(width, h), r.into()),
            t.meter_bg,
        );
        // Where speech should peak.
        let (g0, g1) = (width * db_to_norm(GOOD_DB.0), width * db_to_norm(GOOD_DB.1));
        frame.fill(
            &Path::rectangle(Point::new(g0, 0.0), Size::new(g1 - g0, h)),
            theme::tint(theme::ACCENT, 0.16),
        );
        let w = width * db_norm(self.level);
        if w > 1.0 {
            let color = if self.clipped || self.level >= 0.99 {
                theme::DANGER
            } else if self.muted {
                t.text_dim
            } else if self.level > 0.5 {
                // Above -6 dB: close to the top.
                theme::WARN
            } else {
                theme::ACCENT
            };
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, Size::new(w.max(h), h), r.into()),
                color,
            );
        }
        if self.hold > 1e-4 {
            let hx = (width * db_norm(self.hold)).min(width - 2.0);
            frame.fill(
                &Path::rectangle(Point::new(hx, 0.0), Size::new(2.0, h)),
                t.text,
            );
        }
        if self.scale {
            for db in [-48.0f32, -36.0, -24.0, -12.0, -6.0, 0.0] {
                let x = width * db_to_norm(db);
                let label = format!("{db:.0}");
                frame.fill_text(canvas::Text {
                    content: label,
                    position: Point::new(x.min(width - 1.0), h + 2.0),
                    color: t.text_dim,
                    size: 10.0.into(),
                    align_x: if db >= 0.0 {
                        iced_core::text::Alignment::Right
                    } else {
                        iced_core::text::Alignment::Center
                    },
                    ..canvas::Text::default()
                });
            }
        }
        vec![frame.into_geometry()]
    }
}

pub fn meter<'a, Message: 'a>(
    level: f32,
    hold: f32,
    muted: bool,
    clipped: bool,
    scale: bool,
    mode: theme::Mode,
    width: Length,
) -> iced_core::Element<'a, Message, Theme, iced_renderer::Renderer> {
    cv::Canvas::new(Meter {
        level,
        hold,
        muted,
        clipped,
        scale,
        mode,
    })
    .width(width)
    .height(Length::Fixed(if scale {
        BAR_HEIGHT + SCALE_HEIGHT
    } else {
        BAR_HEIGHT
    }))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_stays_flagged_for_a_few_seconds() {
        let mut l = Levels::default();
        l.push(1.0, 0.2, true, 0.016);
        assert!(l.in_clipped() && !l.out_clipped());
        for _ in 0..60 {
            l.push(0.1, 0.1, true, 0.05);
        }
        assert!(l.in_clipped(), "still flagged after 3 s");
        for _ in 0..30 {
            l.push(0.1, 0.1, true, 0.05);
        }
        assert!(!l.in_clipped(), "cleared after the hold");
    }

    #[test]
    fn the_scale_runs_from_the_floor_to_full_scale() {
        assert_eq!(db_norm(0.0), 0.0);
        assert!((db_norm(1.0) - 1.0).abs() < 1e-6);
        assert!((db_to_norm(-30.0) - 0.5).abs() < 1e-6);
    }
}
