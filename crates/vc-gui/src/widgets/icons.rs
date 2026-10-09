//! Inline SVG line icons, tinted to the current text color.

use crate::Element;
use iced_core::{Color, Length};
use iced_widget::svg;

macro_rules! icon_fn {
    ($name:ident, $file:literal) => {
        pub fn $name<'a>(size: f32, color: Color) -> Element<'a> {
            let handle = svg::Handle::from_memory(
                include_bytes!(concat!("../../assets/icons/", $file)).as_slice(),
            );
            svg(handle)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |_theme, _status| svg::Style { color: Some(color) })
                .into()
        }
    };
}

icon_fn!(home, "home.svg");
icon_fn!(mask, "mask.svg");
icon_fn!(sliders, "sliders.svg");
icon_fn!(mixer, "mixer.svg");
icon_fn!(gear, "gear.svg");
icon_fn!(power, "power.svg");
icon_fn!(headphones, "headphones.svg");
icon_fn!(trash, "trash.svg");
icon_fn!(plus, "plus.svg");
icon_fn!(refresh, "refresh.svg");
icon_fn!(mic, "mic.svg");
icon_fn!(chip, "chip.svg");
icon_fn!(check, "check.svg");
icon_fn!(close, "close.svg");
icon_fn!(star, "star.svg");
icon_fn!(search, "search.svg");
icon_fn!(folder, "folder.svg");
icon_fn!(download, "download.svg");

/// The app logo (full-colour).
pub fn logo<'a>(size: f32) -> Element<'a> {
    let handle = svg::Handle::from_memory(include_bytes!("../../assets/icons/logo.svg").as_slice());
    svg(handle)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}
