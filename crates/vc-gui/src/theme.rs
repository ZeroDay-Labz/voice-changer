//! Design tokens and iced theme for Voice Changer.

use iced_core::theme::{Palette, Theme};
use iced_core::{Border, Color, Shadow, border};
use iced_widget::{button, container, pick_list, progress_bar, slider, text_input, toggler};

pub const ACCENT: Color = Color::from_rgb(0.204, 0.780, 0.588); // #34c796
pub const ACCENT_DIM: Color = Color::from_rgb(0.11, 0.376, 0.298);
pub const DANGER: Color = Color::from_rgb(0.886, 0.329, 0.329);
pub const WARN: Color = Color::from_rgb(0.94, 0.70, 0.25);
/// Secondary palette, used deliberately: AI = indigo, favourites/grit = amber,
/// danger/robot = coral, space/echo = sky, tone = violet.
pub const INDIGO: Color = Color::from_rgb8(124, 140, 255);
pub const AMBER: Color = Color::from_rgb8(240, 184, 74);
pub const CORAL: Color = Color::from_rgb8(255, 122, 107);
pub const SKY: Color = Color::from_rgb8(94, 196, 245);
pub const VIOLET: Color = Color::from_rgb8(181, 140, 255);

/// A translucent tint of `c` for backgrounds.
pub const fn tint(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

pub struct Tokens {
    pub bg: Color,
    pub sidebar: Color,
    pub panel: Color,
    pub raised: Color,
    pub raised_hover: Color,
    pub line: Color,
    pub text: Color,
    pub text_dim: Color,
    pub meter_bg: Color,
}

pub fn tokens(mode: Mode) -> Tokens {
    match mode {
        Mode::Dark => Tokens {
            bg: Color::from_rgb8(17, 19, 24),
            sidebar: Color::from_rgb8(13, 15, 19),
            panel: Color::from_rgb8(26, 29, 36),
            raised: Color::from_rgb8(38, 42, 52),
            raised_hover: Color::from_rgb8(52, 58, 72),
            line: Color::from_rgba8(255, 255, 255, 0.07),
            text: Color::from_rgb8(236, 240, 244),
            text_dim: Color::from_rgb8(150, 158, 172),
            meter_bg: Color::from_rgb8(10, 11, 14),
        },
        Mode::Light => Tokens {
            bg: Color::from_rgb8(243, 245, 248),
            sidebar: Color::from_rgb8(233, 236, 241),
            panel: Color::WHITE,
            raised: Color::from_rgb8(236, 239, 244),
            raised_hover: Color::from_rgb8(226, 231, 238),
            line: Color::from_rgba8(0, 0, 0, 0.08),
            text: Color::from_rgb8(22, 26, 32),
            text_dim: Color::from_rgb8(100, 108, 122),
            meter_bg: Color::from_rgb8(220, 224, 230),
        },
    }
}

pub fn theme(mode: Mode) -> Theme {
    let t = tokens(mode);
    Theme::custom(
        match mode {
            Mode::Dark => "Voice Changer Dark".to_string(),
            Mode::Light => "Voice Changer Light".to_string(),
        },
        Palette {
            background: t.bg,
            text: t.text,
            primary: ACCENT,
            success: ACCENT,
            warning: WARN,
            danger: DANGER,
        },
    )
}

pub fn mode_of(theme: &Theme) -> Mode {
    if theme.palette().background.r > 0.5 {
        Mode::Light
    } else {
        Mode::Dark
    }
}

pub fn radius(r: f32) -> Border {
    Border {
        radius: border::radius(r),
        ..Border::default()
    }
}

// ----------------------------------------------------------------- containers

pub fn panel(theme: &Theme) -> container::Style {
    let t = tokens(mode_of(theme));
    container::Style {
        background: Some(t.panel.into()),
        border: Border {
            radius: border::radius(14),
            width: 1.0,
            color: t.line,
        },
        ..Default::default()
    }
}

pub fn raised(theme: &Theme) -> container::Style {
    let t = tokens(mode_of(theme));
    container::Style {
        background: Some(t.raised.into()),
        border: radius(10.0),
        ..Default::default()
    }
}

pub fn selected_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(ACCENT_DIM.into()),
        border: Border {
            radius: border::radius(12),
            width: 1.5,
            color: ACCENT,
        },
        ..Default::default()
    }
}

pub fn sidebar(theme: &Theme) -> container::Style {
    let t = tokens(mode_of(theme));
    container::Style {
        background: Some(t.sidebar.into()),
        ..Default::default()
    }
}

pub fn pill(theme: &Theme) -> container::Style {
    let t = tokens(mode_of(theme));
    container::Style {
        background: Some(t.raised.into()),
        border: radius(999.0),
        text_color: Some(t.text_dim),
        ..Default::default()
    }
}

// -------------------------------------------------------------------- buttons

fn btn(
    bg: Color,
    hover: Color,
    text: Color,
    r: f32,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => hover,
            button::Status::Disabled => Color {
                a: bg.a * 0.5,
                ..bg
            },
            _ => bg,
        };
        button::Style {
            background: Some(background.into()),
            text_color: if matches!(status, button::Status::Disabled) {
                Color { a: 0.5, ..text }
            } else {
                text
            },
            border: radius(r),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn button_primary(theme: &Theme, status: button::Status) -> button::Style {
    btn(
        ACCENT,
        Color::from_rgb8(72, 220, 170),
        Color::from_rgb8(8, 30, 22),
        10.0,
    )(theme, status)
}

pub fn button_soft(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(mode_of(theme));
    btn(t.raised, t.raised_hover, t.text, 10.0)(theme, status)
}

pub fn button_ghost(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(mode_of(theme));
    let hover = t.raised;
    btn(Color::TRANSPARENT, hover, t.text, 10.0)(theme, status)
}

pub fn button_indigo(theme: &Theme, status: button::Status) -> button::Style {
    btn(INDIGO, Color::from_rgb8(150, 164, 255), Color::WHITE, 10.0)(theme, status)
}

pub fn button_sky(theme: &Theme, status: button::Status) -> button::Style {
    btn(
        SKY,
        Color::from_rgb8(130, 210, 250),
        Color::from_rgb8(8, 30, 40),
        10.0,
    )(theme, status)
}

pub fn button_danger(theme: &Theme, status: button::Status) -> button::Style {
    btn(
        Color::from_rgba8(226, 84, 84, 0.18),
        Color::from_rgba8(226, 84, 84, 0.35),
        DANGER,
        10.0,
    )(theme, status)
}

/// Sidebar navigation entry; `active` highlights the current page in its colour.
pub fn nav_button(active: bool, accent: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(mode_of(theme));
        let base = if active {
            tint(accent, 0.22)
        } else {
            Color::TRANSPARENT
        };
        let hover = if active { tint(accent, 0.28) } else { t.raised };
        let text = if active { t.text } else { t.text_dim };
        btn(base, hover, text, 10.0)(theme, status)
    }
}

/// A panel with a coloured edge.
pub fn panel_accent(accent: Color) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(mode_of(theme));
        container::Style {
            background: Some(t.panel.into()),
            border: Border {
                radius: border::radius(14),
                width: 1.0,
                color: tint(accent, 0.35),
            },
            ..Default::default()
        }
    }
}

/// Wide tooltip panel.
pub fn help_panel(theme: &Theme) -> container::Style {
    let t = tokens(mode_of(theme));
    container::Style {
        background: Some(Color::from_rgb8(30, 34, 44).into()),
        text_color: Some(Color::from_rgb8(236, 240, 244)),
        border: Border {
            radius: border::radius(10),
            width: 1.0,
            color: t.line,
        },
        shadow: Shadow {
            color: Color {
                a: 0.4,
                ..Color::BLACK
            },
            offset: iced_core::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    }
}

/// The big power switch.
pub fn power_button(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(mode_of(theme));
        let (bg, hover, text) = if on {
            (
                ACCENT,
                Color::from_rgb8(72, 220, 170),
                Color::from_rgb8(8, 30, 22),
            )
        } else {
            (t.raised, t.raised_hover, t.text_dim)
        };
        let mut s = btn(bg, hover, text, 18.0)(theme, status);
        s.shadow = Shadow {
            color: Color {
                a: if on { 0.35 } else { 0.0 },
                ..ACCENT
            },
            offset: iced_core::Vector::new(0.0, 6.0),
            blur_radius: 24.0,
        };
        s
    }
}

// -------------------------------------------------------------------- sliders

pub fn slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let t = tokens(mode_of(theme));
    let handle_color = match status {
        slider::Status::Hovered | slider::Status::Dragged => Color::WHITE,
        _ => Color::from_rgb8(230, 234, 240),
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (ACCENT.into(), t.raised.into()),
            width: 6.0,
            border: radius(3.0),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 8.0 },
            background: handle_color.into(),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

pub fn toggler_style(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let t = tokens(mode_of(theme));
    let on = matches!(
        status,
        toggler::Status::Active { is_toggled: true }
            | toggler::Status::Hovered { is_toggled: true }
    );
    toggler::Style {
        background: (if on { ACCENT } else { t.raised_hover }).into(),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Color::WHITE.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: None,
        border_radius: None,
        padding_ratio: 0.2,
    }
}

pub fn text_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let t = tokens(mode_of(theme));
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: t.meter_bg.into(),
        border: Border {
            radius: border::radius(10),
            width: 1.0,
            color: if focused { ACCENT } else { t.line },
        },
        icon: t.text_dim,
        placeholder: t.text_dim,
        value: t.text,
        selection: ACCENT_DIM,
    }
}

pub fn pick_list_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let t = tokens(mode_of(theme));
    let hovered = matches!(
        status,
        pick_list::Status::Hovered | pick_list::Status::Opened { .. }
    );
    pick_list::Style {
        text_color: t.text,
        placeholder_color: t.text_dim,
        handle_color: t.text_dim,
        background: (if hovered { t.raised_hover } else { t.raised }).into(),
        border: radius(10.0),
    }
}

pub fn progress_style(theme: &Theme) -> progress_bar::Style {
    let t = tokens(mode_of(theme));
    progress_bar::Style {
        background: t.meter_bg.into(),
        bar: ACCENT.into(),
        border: radius(6.0),
    }
}
