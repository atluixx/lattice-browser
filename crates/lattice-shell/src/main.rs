//! Lattice v0.2 testable browser (two-process).
//!
//! GPUI window: toolbar (Back/Forward/Reload/Go/bookmarks), address status,
//! page view. The Servo driver runs as a child process per navigation and
//! writes PNGs; this process polls for title lines on its stdout and for
//! PNG updates. Kill-and-respawn stands in for tab lifecycle until the
//! single-binary link issue is resolved. `zenity --entry` stands in for
//! address-bar typing until GPUI text input is built.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use gpui::*;

const BOOKMARKS: &[(&str, &str)] = &[
    ("Example", "https://example.com/"),
    ("Google", "https://www.google.com/"),
    ("YouTube", "https://www.youtube.com/watch?v=aqz-KE-bpKQ"),
    ("WhatsApp", "https://web.whatsapp.com/"),
];

fn driver_binary() -> String {
    std::env::var("LATTICE_DRIVER")
        .unwrap_or_else(|_| "/tmp/lattice-servo-check2/debug/examples/spike_load".to_string())
}

fn chrome_ua() -> String {
    std::env::var("LATTICE_UA").unwrap_or_else(|_| {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            .to_string()
    })
}

fn page_png() -> PathBuf {
    std::env::temp_dir().join("lattice-page.png")
}

/// Spawn the driver for `url`. Returns the child, a title-line receiver,
/// and the PNG path it renders to.
fn spawn_driver(url: &str) -> (Child, Receiver<String>, PathBuf) {
    let out_png = page_png();
    let _ = std::fs::remove_file(&out_png);
    let mut child = Command::new(driver_binary())
        .args([url, &"120".to_string(), &out_png.display().to_string()])
        .env("LATTICE_UA", chrome_ua())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn driver");
    let (tx, rx) = channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.starts_with("title=") {
                    let _ = tx.send(line);
                }
            }
        });
    }
    (child, rx, out_png)
}

fn prompt_url() -> Option<String> {
    let out = Command::new("zenity")
        .args(["--entry", "--title=Lattice", "--text=URL:"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

struct RootView {
    history: Vec<String>,
    index: usize,
    status: String,
    frame: Option<PathBuf>,
    driver: Option<Child>,
    title_rx: Option<Receiver<String>>,
    png: PathBuf,
    png_gen: u64,
    last_mtime: Option<std::time::SystemTime>,
    nav_rx: Receiver<String>,
    nav_tx: Sender<String>,
}

impl RootView {
    fn new(start_url: String, cx: &mut Context<Self>) -> Self {
        let (nav_tx, nav_rx) = channel::<String>();
        let mut view = Self {
            history: Vec::new(),
            index: 0,
            status: "starting…".to_string(),
            frame: None,
            driver: None,
            title_rx: None,
            png: page_png(),
            png_gen: 0,
            last_mtime: None,
            nav_rx,
            nav_tx,
        };
        view.navigate(&start_url);
        let task = cx.spawn(async move |view, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let cont = view
                .update(cx, |view, cx| view.poll(cx))
                .unwrap_or(false);
            if !cont {
                break;
            }
        });
        task.detach();
        view
    }

    fn navigate(&mut self, url: &str) {
        if let Some(mut child) = self.driver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.history.get(self.index).map(|s| s.as_str()) != Some(url) {
            self.history.truncate(self.index + 1);
            self.history.push(url.to_string());
            self.index = self.history.len() - 1;
        }
        self.frame = None;
        self.status = format!("loading {url} …");
        self.last_mtime = None;
        let (child, title_rx, png) = spawn_driver(url);
        self.driver = Some(child);
        self.title_rx = Some(title_rx);
        self.png = png;
    }

    fn go_offset(&mut self, delta: isize) {
        let next = self.index as isize + delta;
        if next >= 0 && (next as usize) < self.history.len() {
            self.index = next as usize;
            let url = self.history[self.index].clone();
            self.navigate(&url);
        }
    }

    /// Returns false when polling should stop (currently never: the window
    /// owns the loop).
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        while let Ok(url) = self.nav_rx.try_recv() {
            self.navigate(&url);
            changed = true;
        }
        if let Some(rx) = self.title_rx.as_ref() {
            while let Ok(line) = rx.try_recv() {
                let title = line.strip_prefix("title=").unwrap_or(&line);
                self.status = format!("{} — {title}", self.history[self.index]);
                changed = true;
            }
        }
        if let Ok(mtime) = std::fs::metadata(&self.png).and_then(|m| m.modified()) {
            if Some(mtime) != self.last_mtime {
                self.png_gen += 1;
                let cached =
                    std::env::temp_dir().join(format!("lattice-frame-{}.png", self.png_gen % 2));
                if std::fs::copy(&self.png, &cached).is_ok() {
                    self.frame = Some(cached);
                    changed = true;
                }
                self.last_mtime = Some(mtime);
            }
        }
        if changed {
            cx.notify();
        }
        true
    }
}

fn btn(
    id: &'static str,
    label: &str,
    cx: &mut Context<RootView>,
    f: impl Fn(&mut RootView, &ClickEvent, &mut Window, &mut Context<RootView>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .mr_1()
        .rounded_md()
        .bg(rgb(0x3a3a3a))
        .cursor_pointer()
        .on_click(cx.listener(f))
        .child(label.to_string())
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.clone();
        let can_back = self.index > 0;
        let can_fwd = self.index + 1 < self.history.len();
        let nav_tx = self.nav_tx.clone();

        let mut bar = div().flex().flex_row().px_2().py_1().bg(rgb(0x2d2d2d));
        bar = bar.child(btn("nav-back", if can_back { "◀" } else { "·" }, cx, move |v, _, _, cx| {
            v.go_offset(-1);
            cx.notify();
        }));
        bar = bar.child(btn("nav-fwd", if can_fwd { "▶" } else { "·" }, cx, move |v, _, _, cx| {
            v.go_offset(1);
            cx.notify();
        }));
        bar = bar.child(btn("nav-reload", "Reload", cx, move |v, _, _, cx| {
            let url = v.history[v.index].clone();
            v.navigate(&url);
            cx.notify();
        }));
        bar = bar.child(btn("nav-go", "Go…", cx, move |_, _, _, _| {
            let tx = nav_tx.clone();
            std::thread::spawn(move || {
                if let Some(url) = prompt_url() {
                    let _ = tx.send(url);
                }
            });
        }));
        for &(name, url) in BOOKMARKS {
            let url = url.to_string();
            bar = bar.child(btn(name, name, cx, move |v, _, _, cx| {
                v.navigate(&url);
                cx.notify();
            }));
        }

        let page: AnyElement = match self.frame.clone() {
            Some(p) => img(p).into_any_element(),
            None => div()
                .flex()
                .items_center()
                .justify_center()
                .size_full()
                .child("rendering…")
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xd4d4d4))
            .size_full()
            .child(bar)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .bg(rgb(0x252525))
                    .child(format!("{status}")),
            )
            .child(div().flex_1().child(page))
    }
}

fn main() {
    tracing_subscriber::fmt::init();
    let start = std::env::args()
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
                let start = start.clone();
                move |_, cx| cx.new(|cx| RootView::new(start.clone(), cx))
            },
        )
        .expect("open window");
    });
}
