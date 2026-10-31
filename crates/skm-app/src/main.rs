mod app;
mod backend;
mod model;
mod theme;

use gpui_kit::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1100.), px(720.)),
                    cx,
                ))),
                window_min_size: Some(size(px(640.), px(400.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("skillsmgr".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| app::Skillsmgr::new(window, cx)),
        )
        .expect("failed to open skillsmgr window");
        cx.activate(true);
    });
}
