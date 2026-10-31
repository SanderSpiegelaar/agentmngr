use gpui_kit::{Hsla, WindowAppearance, rgb};

pub const TEXT_FONT: &str = ".SystemUIFont";
pub const PATH_FONT: &str = "Menlo";

#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub sidebar: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub line: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        let is_dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let color = |light, dark| rgb(if is_dark { dark } else { light }).into();
        Self {
            background: color(0xffffff, 0x1c1e20),
            sidebar: color(0xf3f4f3, 0x222426),
            foreground: color(0x202522, 0xe5e7e6),
            muted: color(0x59635c, 0xa5aaa7),
            line: color(0xdde1de, 0x363a38),
            selected: color(0xe3eee6, 0x303a34),
            accent: color(0x28623d, 0x8aba9c),
        }
    }
}
