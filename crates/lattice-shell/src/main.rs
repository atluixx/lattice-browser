//! Lattice v0.4: tabs, persisted history/session, SIGSTOP'd background tabs.
//!
//! GPUI window: tab strip, toolbar (Back/Forward/Reload/bookmarks),
//! clickable address bar with keyboard editing, status line, page view.
//! Each tab owns a Servo driver child process rendering PNGs to its own
//! file. Background tabs are SIGSTOP'd (zero CPU, swapped out) and
//! SIGCONT'd on switch — the honest version of background throttling.
//! Session (open tabs + active) and visit history persist as JSON under
//! ~/.local/share/lattice/.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use gpui::*;
use lattice_core::history::{HistoryLog, data_file};
use lattice_core::session::{Session, SessionTab};

fn chrome_ua() -> String {
    std::env::var("LATTICE_UA").unwrap_or_else(|_| {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            .to_string()
    })
}

/// Geometry mapping for input forwarding, in window pixels at scale 1.
///
/// GPUI 0.2.2 offers no element-bounds query for listeners, so the page
/// origin is a calibrated constant for the fixed v0.5 layout (232px
/// sidebar + 8px gaps + 1px frame borders). It is verified by screenshot
/// click tests; a window resize breaks it until the resize command path
/// is wired (the driver already accepts `resize`).
/// Displayed image is object-fit Contain of the 1280x800 render.
const DRIVER_W: u32 = 1280;
const DRIVER_H: u32 = 800;
const PAGE_ORIGIN_X: f32 = 250.0;
const PAGE_ORIGIN_Y: f32 = 9.0;
const PAGE_BOX_W: f32 = 1021.0;
const PAGE_BOX_H: f32 = 773.0;

/// Map a window-relative point into render pixels, or None if outside.
fn window_to_page(x: f32, y: f32) -> Option<(f32, f32)> {
    let scale = (PAGE_BOX_W / DRIVER_W as f32).min(PAGE_BOX_H / DRIVER_H as f32);
    let dw = DRIVER_W as f32 * scale;
    let dh = DRIVER_H as f32 * scale;
    let ox = PAGE_ORIGIN_X + (PAGE_BOX_W - dw) / 2.0;
    let oy = PAGE_ORIGIN_Y + (PAGE_BOX_H - dh) / 2.0;
    let px = (x - ox) / scale;
    let py = (y - oy) / scale;
    if px >= 0.0 && py >= 0.0 && px < DRIVER_W as f32 && py < DRIVER_H as f32 {
        Some((px, py))
    } else {
        None
    }
}

fn driver_binary() -> String {
    std::env::var("LATTICE_DRIVER").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.local/bin/lattice-driver")
    })
}

fn tab_outdir(id: u64) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lattice-tab-{id}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[derive(Debug)]
enum DriverEvent {
    Title(Option<String>),
    Load(String),
    Url(String),
    Frame(PathBuf),
    Favicon(PathBuf),
    Bye,
}

/// One persistent Servo renderer. Speaks the lattice-driver JSON protocol.
struct Driver {
    child: Child,
    stdin: ChildStdin,
    _outdir: PathBuf,
}

impl Driver {
    fn spawn(url: &str, outdir: &PathBuf) -> Option<(Self, Receiver<DriverEvent>)> {
        let _ = std::fs::create_dir_all(outdir);
        let err_stdio = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/opencode/lattice-driver-stderr.log")
        {
            Ok(f) => Stdio::from(f),
            Err(_) => Stdio::null(),
        };
        let mut child = Command::new(driver_binary())
            .args([
                &outdir.display().to_string(),
                &DRIVER_W.to_string(),
                &DRIVER_H.to_string(),
                url,
            ])
            .env("LATTICE_UA", chrome_ua())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(err_stdio)
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let (tx, rx) = channel();
        if let Some(stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let v: serde_json::Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let ev = match v.get("ev").and_then(|e| e.as_str()) {
                        Some("title") => DriverEvent::Title(
                            v.get("title").and_then(|t| t.as_str()).map(|s| s.to_string()),
                        ),
                        Some("load") => DriverEvent::Load(
                            v.get("state").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                        ),
                        Some("url") => DriverEvent::Url(
                            v.get("url").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                        ),
                        Some("frame") => DriverEvent::Frame(PathBuf::from(
                            v.get("path").and_then(|s| s.as_str()).unwrap_or(""),
                        )),
                        Some("favicon") => DriverEvent::Favicon(PathBuf::from(
                            v.get("path").and_then(|s| s.as_str()).unwrap_or(""),
                        )),
                        Some("bye") => DriverEvent::Bye,
                        _ => continue,
                    };
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            });
        }
        Some((Self { child, stdin, _outdir: outdir.clone() }, rx))
    }

    fn send(&mut self, line: &str) {
        let _ = writeln!(self.stdin, "{line}");
    }

    fn stop(&mut self) {
        self.send(r#"{"cmd":"quit"}"#);
        let _ = self.child.wait();
    }
}

fn stop_child(child: &Child) {
    // SIGSTOP a live driver: zero CPU, pageable out. Best effort.
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
    short_host(url)
}

/// What the address bar shows when not editing: host only, like the
/// reference (no scheme, path, or query).
fn short_host(url: &str) -> String {
    let mut s = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    for end in [s.find('/'), s.find('?'), s.find('#')].into_iter().flatten() {
        s = &s[..end];
        break;
    }
    s.chars().take(48).collect()
}

struct Tab {
    id: u64,
    history: Vec<String>,
    index: usize,
    title: Option<String>,
    status: String,
    frame: Option<PathBuf>,
    favicon: Option<PathBuf>,
    driver: Option<Driver>,
    ev_rx: Option<Receiver<DriverEvent>>,
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

    fn spawn_tab_driver(&mut self, idx: usize, url: &str) {
        let outdir = tab_outdir(self.tabs[idx].id);
        match Driver::spawn(url, &outdir) {
            Some((driver, rx)) => {
                self.tabs[idx].driver = Some(driver);
                self.tabs[idx].ev_rx = Some(rx);
            }
            None => {
                self.tabs[idx].status = "driver failed to start".to_string();
            }
        }
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
            favicon: None,
            driver: None,
            ev_rx: None,
            addr: url.clone(),
            editing: false,
            id,
        });
        let idx = self.tabs.len() - 1;
        self.spawn_tab_driver(idx, &url);
        idx
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
            t.frame = None;
            t.title = None;
            t.status = format!("loading {url} …");
            t.addr = url.to_string();
            t.editing = false;
            t.favicon = None;
        }
        let cmd = serde_json::json!({"cmd":"navigate","url":url}).to_string();
        if let Some(d) = self.tabs[tab].driver.as_mut() {
            d.send(&cmd);
        } else {
            let url = url.to_string();
            self.spawn_tab_driver(tab, &url);
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
            stop_child(&old.child);
        }
        self.active = idx;
        if let Some(new) = self.tabs[self.active].driver.as_ref() {
            cont_child(&new.child);
        }
        self.save_session();
    }

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        if let Some(mut d) = tab.driver.take() {
            d.stop();
            let _ = d.child.kill();
            let _ = d.child.wait();
        }
        let _ = std::fs::remove_dir_all(tab_outdir(tab.id));
        if self.tabs.is_empty() {
            let i = self.new_tab("https://example.com/".to_string());
            self.active = i;
            return;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if idx < self.active {
            self.active -= 1;
        }
        if let Some(cur) = self.tabs[self.active].driver.as_ref() {
            cont_child(&cur.child);
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
        // Collect events first (borrow ends before mutation below).
        let mut pending: Vec<(u64, DriverEvent)> = Vec::new();
        for tab in self.tabs.iter() {
            if let Some(rx) = tab.ev_rx.as_ref() {
                while let Ok(ev) = rx.try_recv() {
                    pending.push((tab.id, ev));
                }
            }
        }
        for (id, ev) in pending {
            let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
                continue;
            };
            match ev {
                DriverEvent::Title(title) => {
                    if let Some(title) = title {
                        if !title.is_empty() {
                            let url = tab.history[tab.index].clone();
                            tab.title = Some(title.clone());
                            tab.status = format!("{url} — {title}");
                            changed = true;
                        }
                    }
                }
                DriverEvent::Load(state) => {
                    if state == "Complete" {
                        changed = true;
                    }
                }
                DriverEvent::Url(url) => {
                    if url.is_empty() {
                        continue;
                    }
                    if tab.history.get(tab.index).map(|s| s.as_str()) != Some(url.as_str()) {
                        tab.history.truncate(tab.index + 1);
                        tab.history.push(url.clone());
                        tab.index = tab.history.len() - 1;
                        self.history_log.push(&url);
                    }
                    if !tab.editing {
                        tab.addr = url;
                    }
                    changed = true;
                }
                DriverEvent::Frame(path) => {
                    if path.as_os_str().is_empty() {
                        continue;
                    }
                    tab.frame = Some(path);
                    changed = true;
                }
                DriverEvent::Favicon(path) => {
                    if path.as_os_str().is_empty() {
                        continue;
                    }
                    tab.favicon = Some(path);
                    changed = true;
                }
                DriverEvent::Bye => {}
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
            short_host(&tab.history[tab.index])
        };
        let addr_focus = self.addr_focus.clone();

        let mut tabs_col = div().flex().flex_col().gap_1();
        tabs_col = tabs_col.child(
            div()
                .px_2()
                .pt_1()
                .text_xs()
                .text_color(rgba(0xffffff66))
                .child("TABS".to_string()),
        );
        for (i, t) in self.tabs.iter().enumerate() {
            let label = short_title(&t.history[t.index], &t.title);
            let is_active = i == active;
            let icon: AnyElement = match t.favicon.clone() {
                Some(p) => img(p)
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded_md()
                    .overflow_hidden()
                    .into_any_element(),
                None => div().into_any_element(),
            };
            let bg = if is_active {
                rgba(0xffffff2e)
            } else {
                rgba(0xffffff0d)
            };
            tabs_col = tabs_col.child(
                div()
                    .id(1000 + i)
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_3()
                    .py_2()
                    .rounded_xl()
                    .bg(bg)
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| {
                        eprintln!("UI click: tab {i}");
                        v.switch_tab(i);
                        cx.notify();
                    }))
                    .child(icon)
                    .child(div().flex_1().child(label))
                    .child(
                        div()
                            .id(2000 + i)
                            .px_2()
                            .rounded_full()
                            .cursor_pointer()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                eprintln!("UI click: tab-close {i}");
                                v.close_tab(i);
                                cx.notify();
                            }))
                            .child("×".to_string()),
                    ),
            );
        }
        tabs_col = tabs_col.child(btn("newtab", "+ New Tab".to_string(), false, cx, move |v, _, _, cx| {
            let i = v.new_tab("https://example.com/".to_string());
            v.switch_tab(i);
            cx.notify();
        }));

        let mut bar = div().flex().flex_row().items_center().gap_1();
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
        let mut marks = div().flex().flex_col().gap_1();
        marks = marks.child(
            div()
                .px_2()
                .pt_1()
                .text_xs()
                .text_color(rgba(0xffffff66))
                .child("BOOKMARKS".to_string()),
        );
        for &(name, url) in BOOKMARKS {
            let url = url.to_string();
            let initial: String = name.chars().take(1).collect();
            marks = marks.child(
                div()
                    .id(name)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| {
                        eprintln!("UI click: bookmark {name}");
                        let a = v.active;
                        v.navigate(a, &url);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded_full()
                            .bg(rgba(0xffffff1f))
                            .text_xs()
                            .child(initial),
                    )
                    .child(name.to_string()),
            );
        }
        let address = div()
            .id("address-bar")
            .flex()
            .items_center()
            .h(px(34.0))
            .px_3()
            .rounded_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_sm()
            .bg(rgba(0x00000066))
            .overflow_hidden()
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
            .flex_row()
            .bg(rgba(0x00000000))
            .text_color(rgb(0xd4d4d4))
            .size_full()
            .p_2()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(232.0))
                    .p_3()
                    .gap_2()
                    .rounded_2xl()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgba(0xffffff29))
                    .bg(rgba(0x1a1a1acc))
                    .child(bar)
                    .child(address)
                    .child(marks)
                    .child(tabs_col)
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(rgba(0x00000033))
                            .text_xs()
                            .overflow_hidden()
                            .child(format!("{status}")),
                    ),
            )
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
                    .child(
                        div()
                            .id("page-view")
                            .flex_1()
                            .m_2()
                            .rounded_xl()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgba(0xffffff14))
                            .bg(rgba(0x0a0a0af2))
                            .on_scroll_wheel(cx.listener(
                                |view, ev: &ScrollWheelEvent, _, _| {
                                    let (dx, dy) = match ev.delta {
                                        ScrollDelta::Pixels(p) => {
                                            (f64::from(p.x), f64::from(p.y))
                                        }
                                        ScrollDelta::Lines(p) => (p.x as f64 * 20.0, p.y as f64 * 20.0),
                                    };
                                    // Scroll position barely matters; use
                                    // the viewport center.
                                    let cmd = serde_json::json!({
                                        "cmd": "scroll",
                                        "dx": -dx,
                                        "dy": -dy,
                                        "x": DRIVER_W as f32 / 2.0,
                                        "y": DRIVER_H as f32 / 2.0,
                                    })
                                    .to_string();
                                    let a = view.active;
                                    if let Some(d) = view.tabs.get_mut(a).and_then(|t| t.driver.as_mut()) {
                                        d.send(&cmd);
                                    }
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, ev: &MouseDownEvent, _, _| {
                                    let p = ev.position;
                                    if let Some((x, y)) =
                                        window_to_page(f32::from(p.x), f32::from(p.y))
                                    {
                                        eprintln!("UI page click {x:.0},{y:.0}");
                                        let cmd = serde_json::json!({"cmd":"click","x":x,"y":y})
                                            .to_string();
                                        let a = view.active;
                                        if let Some(d) = view.tabs.get_mut(a).and_then(|t| t.driver.as_mut()) {
                                            d.send(&cmd);
                                        }
                                    }
                                }),
                            )
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
                        // Clamp active and spawn one persistent driver per
                        // tab; only the active tab renders, the rest
                        // start stopped.
                        if view.active >= view.tabs.len() {
                            view.active = 0;
                        }
                        for i in 0..view.tabs.len() {
                            let url = view.tabs[i].history[0].clone();
                            view.spawn_tab_driver(i, &url);
                            if i != view.active {
                                if let Some(c) = view.tabs[i].driver.as_ref() {
                                    stop_child(&c.child);
                                }
                            }
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
