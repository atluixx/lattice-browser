//! Lattice M1: GPUI window showing a live Servo-rendered page.
//!
//! Without the `servo` feature this is the blank fast-startup shell.
//! With `servo`, a driver thread renders offscreen and the UI uploads each
//! settled frame as a GPUI texture (`ImageSource::Custom`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::*;

#[cfg(feature = "servo")]
mod page;

struct RootView {
    url: String,
    status: String,
    #[cfg(feature = "servo")]
    rx: std::sync::mpsc::Receiver<page::FrameMsg>,
    image: Arc<Mutex<Option<Arc<RenderImage>>>>,
}

impl RootView {
    fn new(url: String, cx: &mut Context<Self>) -> Self {
        #[cfg(feature = "servo")]
        let rx = {
            let (tx, rx) = std::sync::mpsc::channel();
            page::spawn_servo(url.clone(), tx);
            let task = cx.spawn(async move |view, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    let alive = view
                        .update(cx, |view, cx| view.poll())
                        .unwrap_or(true);
                    if alive {
                        break;
                    }
                }
            });
            task.detach();
            rx
        };

        Self {
            url,
            status: "loading…".to_string(),
            #[cfg(feature = "servo")]
            rx,
            image: Arc::new(Mutex::new(None)),
        }
    }

    /// Drain pending frames into the texture slot. Returns true when the
    /// driver thread is gone and polling should stop.
    #[cfg(feature = "servo")]
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let mut updated = false;
        loop {
            match self.rx.try_recv() {
                Ok(msg) => {
                    if !msg.pixels.is_empty() {
                        let mut px = msg.pixels;
                        for chunk in px.chunks_exact_mut(4) {
                            chunk.swap(0, 2); // RGBA -> BGRA for RenderImage
                        }
                        if let Some(buf) =
                            image::RgbaImage::from_raw(msg.width, msg.height, px)
                        {
                            let render =
                                RenderImage::new(vec![image::Frame::new(buf)]);
                            *self.image.lock().unwrap() = Some(Arc::new(render));
                            updated = true;
                        }
                    }
                    let title = msg.title.unwrap_or_default();
                    self.status = if title.is_empty() {
                        msg.status.clone()
                    } else {
                        format!("{title} — {}", msg.status)
                    };
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if updated {
                        cx.notify();
                    }
                    return true;
                }
            }
        }
        if updated {
            cx.notify();
        }
        false
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let slot = self.image.clone();
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
                    .child(format!("Lattice M1 — {} — {}", self.url, self.status)),
            )
            .child(div().flex_1().child(img(ImageSource::Custom(Arc::new(
                move |_window: &mut Window, _cx: &mut App| {
                    slot.lock().unwrap().clone().map(Ok)
                },
            )))))
    }
}

fn main() {
    tracing_subscriber::fmt::init();
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/".to_string());
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                let url = url.clone();
                move |_, cx| {
                    cx.new(|cx| RootView::new(url.clone(), cx))
                }
            },
        )
        .expect("open window");
    });
}
