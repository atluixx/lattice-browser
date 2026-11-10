//! Lattice v0.3 testable browser (two-process).
//!
//! GPUI window: toolbar (Back/Forward/Reload/bookmarks), clickable address
//! bar with keyboard editing (Enter commits, Esc cancels), status line,
//! page view. The Servo driver runs as a child process per navigation and
//! writes PNGs; this process polls for titles on its stdout and for PNG
//! updates. Kill-and-respawn stands in for tab lifecycle until the
//! single-binary link issue is resolved.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use gpui::*;

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
    let err_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/opencode/lattice-driver-stderr.log")
        .ok();
    let mut child = Command::new(driver_binary())
        .args([url, &"60".to_string(), &out_png.display().to_string()])
        .env("LATTICE_UA", chrome_ua())
        .stdout(Stdio::piped())
        .stderr(err_log.map_or(Stdio::null(), Stdio::from))
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

fn normalize_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") {
        Some(s.to_string())
    } else {
        Some(format!("https://{s}"))
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
    nav_id: u64,
    last_mtime: Option<std::time::SystemTime>,
    addr: String,
    editing: bool,
    addr_focus: FocusHandle,
}

impl RootView {
    fn new(start_url: String, cx: &mut Context<Self>) -> Self {
        let addr_focus = cx.focus_handle();
        let mut view = Self {
            history: Vec::new(),
            index: 0,
            status: "starting…".to_string(),
            frame: None,
            driver: None,
            title_rx: None,
            png: page_png(),
            png_gen: 0,
            nav_id: 0,
            last_mtime: None,
            addr: start_url.clone(),
            editing: false,
            addr_focus,
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
        self.addr = url.to_string();
        self.editing = false;
        self.last_mtime = None;
        self.nav_id += 1;
        // Drop the previous navigation's frame file so a torn copy can never
        // resurface as stale content.
        let _ = std::fs::remove_file(
            std::env::temp_dir().join(format!("lattice-frame-{}.png", self.nav_id)),
        );
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

    fn commit_address(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = normalize_url(&self.addr.clone()) {
            self.navigate(&url);
        } else {
            self.addr = self.history[self.index].clone();
            self.editing = false;
        }
        cx.notify();
    }

    /// Returns false when polling should stop (currently never: the window
    /// owns the loop).
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if let Some(rx) = self.title_rx.as_ref() {
            while let Ok(line) = rx.try_recv() {
                let title = line.strip_prefix("title=").unwrap_or(&line);
                if !title.is_empty() {
                    self.status = format!("{} — {title}", self.history[self.index]);
                    changed = true;
                }
            }
        }
        if let Ok(mtime) = std::fs::metadata(&self.png).and_then(|m| m.modified()) {
            if Some(mtime) != self.last_mtime {
                self.png_gen += 1;
                // One frame file per navigation: a failed/torn copy can only
                // leave this navigation's slot empty ("rendering…"), never
                // show another page's pixels.
                let cached = std::env::temp_dir()
                    .join(format!("lattice-frame-{}.png", self.nav_id));
                if std::fs::copy(&self.png, &cached).is_ok() {
                    self.frame = Some(cached);
                    if self.status.starts_with("loading ") {
                        self.status = self.history[self.index].clone();
                    }
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
        .on_click(cx.listener(move |v, e, w, cx| {
            eprintln!("UI click: {id}");
            f(v, e, w, cx)
        }))
        .child(label.to_string())
}

const BOOKMARKS: &[(&str, &str)] = &[
    ("Example", "https://example.com/"),
    ("Google", "https://www.google.com/"),
    ("YouTube", "https://www.youtube.com/watch?v=aqz-KE-bpKQ"),
    ("WhatsApp", "https://web.whatsapp.com/"),
];

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.clone();
        let can_back = self.index > 0;
        let can_fwd = self.index + 1 < self.history.len();
        let addr_text = if self.editing {
            format!("{}▏", self.addr)
        } else {
            self.history.get(self.index).cloned().unwrap_or_default()
        };
        let addr_focus = self.addr_focus.clone();

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
        for &(name, url) in BOOKMARKS {
            let url = url.to_string();
            bar = bar.child(btn(name, name, cx, move |v, _, _, cx| {
                v.navigate(&url);
                cx.notify();
            }));
        }
        let address = div()
            .id("address-bar")
            .flex_1()
            .px_2()
            .py_1()
            .ml_1()
            .rounded_md()
            .bg(rgb(0x1a1a1a))
            .cursor_text()
            .track_focus(&addr_focus)
            .on_click(cx.listener(move |view, _, window: &mut Window, cx| {
                eprintln!("UI address click");
                view.editing = true;
                view.addr = view.history[view.index].clone();
                window.focus(&addr_focus);
                cx.notify();
            }))
            .on_key_down(cx.listener(|view, ev: &KeyDownEvent, _, cx| {
                eprintln!("UI key: {}", ev.keystroke.key);
                let ks = &ev.keystroke;
                if ks.modifiers.control || ks.modifiers.platform {
                    return;
                }
                match ks.key.as_str() {
                    "Enter" => view.commit_address(cx),
                    "Escape" => {
                        view.editing = false;
                        view.addr = view.history[view.index].clone();
                        cx.notify();
                    }
                    "Backspace" => {
                        view.addr.pop();
                        cx.notify();
                    }
                    _ => {
                        if let Some(ch) = ks.key_char.as_deref() {
                            if ch != "\n" {
                                view.addr.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }
            }))
            .child(addr_text);

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
            .child(address)
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
