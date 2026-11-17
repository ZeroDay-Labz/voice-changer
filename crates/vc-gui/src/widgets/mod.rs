pub mod icons;
pub mod knob;
pub mod meter;
pub mod slider;

/// Mouse-wheel handling shared by the knobs and sliders.
///
/// A wheel turn that started as scrolling the page must not change a
/// control the pointer happens to pass over. So a control only takes the
/// wheel when the pointer was already resting on it: a turn that arrives
/// within a moment of scrolling elsewhere is left to the page.
#[derive(Default)]
pub struct Wheel {
    last_page_scroll: Option<std::time::Instant>,
    /// Shift is held: fine steps.
    pub fine: bool,
}

impl Wheel {
    const LATCH: std::time::Duration = std::time::Duration::from_millis(500);

    /// Track Shift. Call with every event.
    pub fn observe(&mut self, event: &iced_core::Event) {
        if let iced_core::Event::Keyboard(iced_core::keyboard::Event::ModifiersChanged(m)) = event {
            self.fine = m.shift();
        }
    }

    /// For a wheel event: how far to move the control (normalized), or
    /// `None` when the turn belongs to the page.
    pub fn step(&mut self, delta: &iced_core::mouse::ScrollDelta, over: bool) -> Option<f32> {
        let now = std::time::Instant::now();
        let scrolling_page = self
            .last_page_scroll
            .is_some_and(|t| now.duration_since(t) < Self::LATCH);
        if !over || scrolling_page {
            self.last_page_scroll = Some(now);
            return None;
        }
        let lines = match delta {
            iced_core::mouse::ScrollDelta::Lines { y, .. } => *y,
            iced_core::mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
        };
        Some(lines * if self.fine { 0.004 } else { 0.02 })
    }
}

#[cfg(test)]
mod tests {
    use super::Wheel;
    use iced_core::mouse::ScrollDelta;

    #[test]
    fn a_page_scroll_passing_over_a_control_does_not_change_it() {
        let turn = ScrollDelta::Lines { x: 0.0, y: 1.0 };
        let mut w = Wheel::default();
        // Resting on the control: the wheel adjusts it.
        assert!(w.step(&turn, true).is_some());
        // Scrolling the page, then the pointer ends up over the control.
        let mut w = Wheel::default();
        assert!(w.step(&turn, false).is_none());
        assert!(w.step(&turn, true).is_none());
        assert!(w.step(&turn, true).is_none());
        // After a pause it is the control's turn again.
        std::thread::sleep(Wheel::LATCH + std::time::Duration::from_millis(30));
        assert!(w.step(&turn, true).is_some());
    }

    #[test]
    fn shift_makes_fine_steps() {
        let turn = ScrollDelta::Lines { x: 0.0, y: 1.0 };
        let mut w = Wheel::default();
        let coarse = w.step(&turn, true).unwrap();
        w.fine = true;
        let fine = w.step(&turn, true).unwrap();
        assert!(fine < coarse / 3.0);
    }
}
