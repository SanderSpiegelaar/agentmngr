use gpui_kit::{
    AppContext, Context, IntoElement, Render, TitlebarOptions, Window, WindowOptions, div,
};

struct Skillsmgr;

impl Render for Skillsmgr {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("skillsmgr".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |_, cx| cx.new(|_| Skillsmgr),
        )
        .expect("failed to open skillsmgr window");
        cx.activate(true);
    });
}
