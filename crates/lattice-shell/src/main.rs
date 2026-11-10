use gpui::*;
use lattice_core::Tab;

struct RootView {
    tabs: Vec<Tab>,
    status: String,
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xd4d4d4))
            .size_full()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .bg(rgb(0x2d2d2d))
                    .child(format!("Lattice M0 — {} tab(s) — {}", self.tabs.len(), self.status)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("blank new tab (fast startup path)"),
            )
    }
}

fn main() {
    tracing_subscriber::fmt::init();
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1024.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| RootView {
                    tabs: vec![Tab::new(1, "about:blank".to_string())],
                    status: lattice_embed::backend_name().to_string(),
                })
            },
        )
        .expect("open window");
    });
}
