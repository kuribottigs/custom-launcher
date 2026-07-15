//! Oxide Launcher — Modrinth-like Minecraft custom launcher built on gpui.

mod bridge;
mod root;
mod text_input;
mod theme;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    prelude::*, px, size,
};

use crate::root::RootView;

actions!(oxide, [Quit]);

fn main() {
    Application::new().run(|cx: &mut App| {
        text_input::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1000.), px(680.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Oxide Launcher".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(RootView::new),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
