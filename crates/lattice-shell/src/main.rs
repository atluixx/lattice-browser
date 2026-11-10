//! Lattice v0.4: tabs, persisted history/session, SIGSTOP'd background tabs.
//!
//! GPUI window: tab strip, toolbar (Back/Forward/Reload/bookmarks),
//! clickable address bar with keyboard editing, status line, page view.
//! Each tab owns a Servo driver child process rendering PNGs to its own
//! file. Background tabs are SIGSTOP'd (zero CPU, swapped out) and
//! SIGCONT'd on switch — the honest version of background throttling.
//! Session (open tabs + active) and visit history persist as JSON under
//! ~/.local/share/lattice/.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use gpui::*;
use lattice_core::history::{HistoryLog, data_file};
use lattice_core::session::{Session, SessionTab};

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

fn tab_png(id: u64) -> PathBuf {
    std::env::temp_dir().join(format!("lattice-page-{id}.png"))
}

fn frame_png(id: u64, nav: u64) -> PathBuf {
    std::env::temp_dir().join(format!("lattice-frame-{id}-{nav}.png"))
}

/// Spawn the Servo driver for `url`, rendering to `out_png`.
/// Returns the child and a title-line receiver.
fn spawn_driver(url: &str, out_png: &PathBuf) -> (Child, Receiver<String>) {
    let _ = std::fs::remove_file(out_png);
    let err_stdio = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/opencode/lattice-driver-stderr.log")
    {
        Ok(f) => Stdio::from(f),
        Err(_) => Stdio::null(),
    };
    let mut child = Command::new(driver_binary())
        .args([url, &"60".to_string(), &out_png.display().to_string()])
        .env("LATTICE_UA", chrome_ua())
        .stdout(Stdio::piped())
        .stderr(err_stdio)
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
    (child, rx)
}

fn stop_child(child: &Child) {
    // SIGSTOP the whole driver tree state: zero CPU, pageable out.
    // Best effort; the Child handle keeps it reaped later via wait().
    unsafe {
        libc::kill(child.id() as i32, libc::SIGSTOP);
    }
}

fn cont_child(child: &Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGCONT);
    }
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

fn short_title(url: &str, title: &Option<String>) -> String {
    if let Some(t) = title {
        let t: String = t.chars().take(24).collect();
        if !t.is_empty() {
            return t;
        }
    }
    let mut s = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if let Some(end) = s.find('/') {
        s = &s[..end];
    }
    s.chars().take(24).collect()
}

struct Tab {
    id: u64,
    history: Vec<String>,
    index: usize,
    title: Option<String>,
    status: String,
    frame: Option<PathBuf>,
    driver: Option<Child>,
    title_rx: Option<Receiver<String>>,
    png: PathBuf,
    nav_id: u64,
    last_mtime: Option<std::time::SystemTime>,
    addr: String,
    editing: bool,
}

struct RootView {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    addr_focus: FocusHandle,
    history_log: HistoryLog,
}

impl RootView {
    fn session_path() -> PathBuf {
        data_file("session.json")
    }

    fn save_session(&self) {
        let tabs = self
            .tabs
            .iter()
            .map(|t| SessionTab {
                url: t.history[t.index].clone(),
                title: t.title.clone().unwrap_or_default(),
            })
            .collect();
        let session = Session {
            tabs,
            active: self.active,
        };
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            let _ = std::fs::write(Self::session_path(), json);
        }
        self.history_log.save(&data_file("history.json"));
    }

    fn new_tab(&mut self, url: String) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            history: vec![url.clone()],
            index: 0,
            title: None,
            status: format!("loading {url} …"),
            frame: None,
            driver: None,
            title_rx: None,
            png: tab_png(id),
            nav_id: 0,
            last_mtime: None,
            addr: url.clone(),
            editing: false,
            id,
        });
        self.tabs.len() - 1
    }

    fn navigate(&mut self, tab: usize, url: &str) {
        if self.tabs[tab].history.get(self.tabs[tab].index).map(|s| s.as_str()) != Some(url) {
            let idx = self.tabs[tab].index;
            self.tabs[tab].history.truncate(idx + 1);
            self.tabs[tab].history.push(url.to_string());
            self.tabs[tab].index = self.tabs[tab].history.len() - 1;
        }
        {
            let t = &mut self.tabs[tab];
            if let Some(mut child) = t.driver.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            t.frame = None;
            t.title = None;
            t.status = format!("loading {url} …");
            t.addr = url.to_string();
            t.editing = false;
            t.last_mtime = None;
            t.nav_id += 1;
            let _ = std::fs::remove_file(frame_png(t.id, t.nav_id));
            let (child, title_rx) = spawn_driver(url, &t.png.clone());
            t.driver = Some(child);
            t.title_rx = Some(title_rx);
        }
        self.history_log.push(url);
        self.save_session();
    }

    fn go_offset(&mut self, delta: isize) {
        let tab = self.active;
        let next = self.tabs[tab].index as isize + delta;
        if next >= 0 && (next as usize) < self.tabs[tab].history.len() {
            self.tabs[tab].index = next as usize;
            let url = self.tabs[tab].history[self.tabs[tab].index].clone();
            self.navigate(tab, &url);
        }
    }

    fn switch_tab(&mut self, idx: usize) {
        if idx == self.active || idx >= self.tabs.len() {
            return;
        }
        if let Some(old) = self.tabs[self.active].driver.as_ref() {
            stop_child(old);
        }
        self.active = idx;
        if let Some(new) = self.tabs[self.active].driver.as_ref() {
            cont_child(new);
        }
        self.save_session();
    }

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        if let Some(mut child) = tab.driver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&tab.png);
        if self.tabs.is_empty() {
            let i = self.new_tab("https://example.com/".to_string());
            self.active = i;
            let url = self.tabs[i].history[0].clone();
            self.navigate(i, &url);
            return;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if idx < self.active {
            self.active -= 1;
        }
        if let Some(cur) = self.tabs[self.active].driver.as_ref() {
            cont_child(cur);
        }
        self.save_session();
    }

    fn commit_address(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        if let Some(url) = normalize_url(&self.tabs[active].addr.clone()) {
            self.navigate(active, &url);
        } else {
            let cur = self.tabs[active].history[self.tabs[active].index].clone();
            self.tabs[active].addr = cur;
            self.tabs[active].editing = false;
        }
        cx.notify();
    }

    /// Returns false when polling should stop (currently never: the window
    /// owns the loop).
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for tab in self.tabs.iter_mut() {
            if let Some(rx) = tab.title_rx.as_ref() {
                while let Ok(line) = rx.try_recv() {
                    let title = line.strip_prefix("title=").unwrap_or(&line);
                    if !title.is_empty() {
                        tab.title = Some(title.to_string());
                        tab.status =
                            format!("{} — {title}", tab.history[tab.index]);
                        changed = true;
                    }
                }
            }
        }
        // Only the active tab's pixels are composited.
        let active = self.active;
        if let Some(tab) = self.tabs.get_mut(active) {
            if let Ok(mtime) = std::fs::metadata(&tab.png).and_then(|m| m.modified()) {
                if Some(mtime) != tab.last_mtime {
                    let cached = frame_png(tab.id, tab.nav_id);
                    if std::fs::copy(&tab.png, &cached).is_ok() {
                        tab.frame = Some(cached);
                        changed = true;
                    }
                    tab.last_mtime = Some(mtime);
                }
            }
        }
        if changed {
            // Retitle the newest untitled visit for any tab that got a title.
            for tab in self.tabs.iter() {
                if let Some(t) = tab.title.clone() {
                    self.history_log
                        .retitle(&tab.history[tab.index], &t);
                }
            }
            self.save_session();
            cx.notify();
        }
        true
    }
}

fn btn(
    id: impl Into<ElementId>,
    label: String,
    active: bool,
    cx: &mut Context<RootView>,
    f: impl Fn(&mut RootView, &ClickEvent, &mut Window, &mut Context<RootView>) + 'static,
) -> impl IntoElement {
    let bg = if active {
        rgba(0xffffff2e)
    } else {
        rgba(0xffffff14)
    };
    let log_label = label.clone();
    div()
        .id(id)
        .px_3()
        .py_1()
        .mr_1()
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(rgba(0xffffff1f))
        .cursor_pointer()
        .on_click(cx.listener(move |v, e, w, cx| {
            eprintln!("UI click: {log_label}");
            f(v, e, w, cx)
        }))
        .child(label)
}

const BOOKMARKS: &[(&str, &str)] = &[
    ("Example", "https://example.com/"),
    ("Google", "https://www.google.com/"),
    ("YouTube", "https://www.youtube.com/watch?v=aqz-KE-bpKQ"),
    ("WhatsApp", "https://web.whatsapp.com/"),
];

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active;
        let tab = &self.tabs[active];
        let status = tab.status.clone();
        let can_back = tab.index > 0;
        let can_fwd = tab.index + 1 < tab.history.len();
        let addr_text = if tab.editing {
            format!("{}▏", tab.addr)
        } else {
            tab.history[tab.index].clone()
        };
        let addr_focus = self.addr_focus.clone();

        let mut tabbar = div().flex().flex_row().items_center().px_2().pt_2();
        for (i, t) in self.tabs.iter().enumerate() {
            let label = short_title(&t.history[t.index], &t.title);
            let is_active = i == active;
            tabbar = tabbar.child(btn(
                1000 + i,
                if is_active {
                    format!("● {label}")
                } else {
                    format!("○ {label}")
                },
                is_active,
                cx,
                move |v, _, _, cx| {
                    v.switch_tab(i);
                    cx.notify();
                },
            ));
            tabbar = tabbar.child(btn(2000 + i, "×".to_string(), false, cx, move |v, _, _, cx| {
                v.close_tab(i);
                cx.notify();
            }));
        }
        tabbar = tabbar.child(btn("newtab", "+".to_string(), false, cx, move |v, _, _, cx| {
            let i = v.new_tab("https://example.com/".to_string());
            let url = v.tabs[i].history[0].clone();
            v.switch_tab(i);
            v.navigate(i, &url);
            cx.notify();
        }));

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .py_2()
            .m_2()
            .rounded_2xl()
            .bg(rgba(0x1e1e1ecc))
            .border_1()
            .border_color(rgba(0xffffff1f));
        bar = bar.child(btn("nav-back", if can_back { "◀" } else { "·" }.to_string(), false, cx, move |v, _, _, cx| {
            v.go_offset(-1);
            cx.notify();
        }));
        bar = bar.child(btn("nav-fwd", if can_fwd { "▶" } else { "·" }.to_string(), false, cx, move |v, _, _, cx| {
            v.go_offset(1);
            cx.notify();
        }));
        bar = bar.child(btn("nav-reload", "Reload".to_string(), false, cx, move |v, _, _, cx| {
            let url = v.tabs[v.active].history[v.tabs[v.active].index].clone();
            let a = v.active;
            v.navigate(a, &url);
            cx.notify();
        }));
        for &(name, url) in BOOKMARKS {
            let url = url.to_string();
            bar = bar.child(btn(name, name.to_string(), false, cx, move |v, _, _, cx| {
                let a = v.active;
                v.navigate(a, &url);
                cx.notify();
            }));
        }
        let address = div()
            .id("address-bar")
            .flex_1()
            .px_3()
            .py_1()
            .ml_1()
            .rounded_full()
            .bg(rgba(0x00000066))
            .border_1()
            .border_color(rgba(0xffffff14))
            .cursor_text()
            .track_focus(&addr_focus)
            .on_click(cx.listener(move |view, _, window: &mut Window, cx| {
                eprintln!("UI address click");
                let a = view.active;
                view.tabs[a].editing = true;
                let cur = view.tabs[a].history[view.tabs[a].index].clone();
                view.tabs[a].addr = cur;
                window.focus(&addr_focus);
                cx.notify();
            }))
            .on_key_down(cx.listener(|view, ev: &KeyDownEvent, _, cx| {
                eprintln!(
                    "UI key: key={:?} char={:?} mods=c{} s{} a{} p{}",
                    ev.keystroke.key,
                    ev.keystroke.key_char,
                    ev.keystroke.modifiers.control,
                    ev.keystroke.modifiers.shift,
                    ev.keystroke.modifiers.alt,
                    ev.keystroke.modifiers.platform
                );
                let ks = &ev.keystroke;
                if ks.modifiers.control || ks.modifiers.platform {
                    return;
                }
                let a = view.active;
                // NOTE: GPUI key names are lowercase ("enter", "backspace").
                match ks.key.to_lowercase().as_str() {
                    "enter" | "return" | "kp_enter" => view.commit_address(cx),
                    "escape" => {
                        let cur = view.tabs[a].history[view.tabs[a].index].clone();
                        view.tabs[a].addr = cur;
                        view.tabs[a].editing = false;
                        cx.notify();
                    }
                    "backspace" => {
                        view.tabs[a].addr.pop();
                        cx.notify();
                    }
                    _ => {
                        if let Some(ch) = ks.key_char.as_deref() {
                            if ch != "\n" {
                                view.tabs[a].addr.push_str(ch);
                                cx.notify();
                            }
                        }
                    }
                }
            }))
            .child(addr_text);

        let page: AnyElement = match self.tabs[active].frame.clone() {
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
            .bg(rgba(0x00000000))
            .text_color(rgb(0xd4d4d4))
            .size_full()
            .p_2()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .rounded_2xl()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xffffff29))
                    .bg(rgba(0x141414e6))
                    .child(tabbar)
                    .child(bar)
                    .child(address)
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(rgba(0x00000033))
                            .text_xs()
                            .child(format!("{status}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .m_2()
                            .rounded_xl()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgba(0xffffff14))
                            .bg(rgba(0x0a0a0af2))
                            .child(page),
                    ),
            )
    }
}

fn main() {
    tracing_subscriber::fmt::init();
    let start_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/".to_string());

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            {
                let start_url = start_url.clone();
                move |_, cx| {
                    cx.new(|cx| {
                        // Restore session or start fresh.
                        let history_log = HistoryLog::load(&data_file("history.json"));
                        let mut view = RootView {
                            tabs: Vec::new(),
                            active: 0,
                            next_id: 1,
                            addr_focus: cx.focus_handle(),
                            history_log,
                        };
                        let mut restored = false;
                        if let Ok(data) = std::fs::read_to_string(RootView::session_path()) {
                            if let Ok(session) =
                                serde_json::from_str::<Session>(&data)
                            {
                                for (i, t) in session.tabs.iter().enumerate() {
                                    let idx = view.new_tab(t.url.clone());
                                    view.tabs[idx].title = if t.title.is_empty() {
                                        None
                                    } else {
                                        Some(t.title.clone())
                                    };
                                    if i == session.active {
                                        view.active = idx;
                                    }
                                }
                                if !view.tabs.is_empty() {
                                    restored = true;
                                }
                            }
                        }
                        if !restored {
                            view.new_tab(start_url.clone());
                        }
                        // Clamp active and start drivers: only the active
                        // tab renders; the rest start stopped.
                        if view.active >= view.tabs.len() {
                            view.active = 0;
                        }
                        let ids: Vec<u64> = view.tabs.iter().map(|t| t.id).collect();
                        for (i, id) in ids.iter().enumerate() {
                            let url = view.tabs[i].history[0].clone();
                            view.navigate(i, &url);
                            if i != view.active {
                                if let Some(c) = view.tabs[i].driver.as_ref() {
                                    stop_child(c);
                                }
                            }
                            let _ = id;
                        }
                        // Pump loop: titles, PNGs, and session saves.
                        let task = cx.spawn(async move |view, cx| loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(200))
                                .await;
                            let cont = view
                                .update(cx, |view: &mut RootView, cx| view.poll(cx))
                                .unwrap_or(false);
                            if !cont {
                                break;
                            }
                        });
                        task.detach();
                        view
                    })
                }
            },
        )
        .expect("open window");
    });
}
