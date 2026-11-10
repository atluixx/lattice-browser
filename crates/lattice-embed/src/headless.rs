//! Windowless Servo driver shared by the spike examples and (later) the shell.
//!
//! One `HeadlessSession` owns a single `Servo` instance (single-process,
//! per the product spec) and can host multiple `HeadlessPage`s, each with
//! its own `SoftwareRenderingContext`. Pixels cross to the caller as
//! `image::RgbaImage` — the shell uploads them to a GPUI texture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Once;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::{
    EventLoopWaker, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate,
};
use webrender_api::units::{DeviceIntPoint, DeviceIntRect, DeviceIntSize};

pub use servo::LoadStatus;

static RUSTLS_ONCE: Once = Once::new();

fn ensure_rustls() {
    RUSTLS_ONCE.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("rustls provider");
    });
}

#[derive(Clone)]
struct Waker {
    tx: std::sync::mpsc::Sender<()>,
}

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self.tx.send(());
    }
}

struct Delegate {
    title: RefCell<Option<String>>,
    status_text: RefCell<Option<String>>,
    status: Cell<LoadStatus>,
    frames: Cell<u32>,
}

impl Delegate {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            title: RefCell::new(None),
            status_text: RefCell::new(None),
            status: Cell::new(LoadStatus::Started),
            frames: Cell::new(0),
        })
    }
}

impl WebViewDelegate for Delegate {
    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        *self.title.borrow_mut() = title;
    }

    fn notify_status_text_changed(&self, _webview: WebView, status: Option<String>) {
        *self.status_text.borrow_mut() = status;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.status.set(status);
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.frames.set(self.frames.get() + 1);
    }
}

/// One Servo instance on the current thread. Not `Send`: drive it from a
/// dedicated thread and move pixels out (matches the spec's threading model).
pub struct HeadlessSession {
    servo: Servo,
    rx: std::sync::mpsc::Receiver<()>,
}

impl HeadlessSession {
    pub fn new() -> Self {
        ensure_rustls();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let mut prefs = servo::Preferences::default();
        if let Ok(ua) = std::env::var("LATTICE_UA") {
            prefs.user_agent = ua;
        }
        let servo = ServoBuilder::default()
            .preferences(prefs)
            .event_loop_waker(Box::new(Waker { tx }))
            .build();
        servo.setup_logging();
        Self { servo, rx }
    }

    pub fn user_agent(&self) -> String {
        servo::prefs::get().user_agent.clone()
    }

    pub fn page(&self, url: &str, width: u32, height: u32) -> HeadlessPage {
        let url = url::Url::parse(url).expect("valid URL");
        let context: Rc<dyn RenderingContext> = Rc::new(
            SoftwareRenderingContext::new(PhysicalSize::new(width, height))
                .expect("software rendering context"),
        );
        let delegate = Delegate::new();
        let webview = WebViewBuilder::new(&self.servo, context.clone())
            .url(url)
            .delegate(delegate.clone())
            .build();
        webview.show();
        HeadlessPage {
            webview,
            context,
            delegate,
            width,
            height,
        }
    }

    fn pump_once(&self, wait: Duration) {
        self.servo.spin_event_loop();
        let _ = self.rx.recv_timeout(wait);
        self.servo.spin_event_loop();
    }

    /// Spin until every page reports Complete or the timeout hits.
    pub fn pump_until_complete(&self, pages: &[HeadlessPage], timeout: Duration) {
        let start = Instant::now();
        loop {
            self.pump_once(Duration::from_millis(50));
            let done = pages
                .iter()
                .all(|p| p.delegate.status.get() == LoadStatus::Complete);
            if done || start.elapsed() >= timeout {
                break;
            }
        }
    }
}

/// One loaded page: its view, pixels, and load report.
pub struct HeadlessPage {
    webview: WebView,
    context: Rc<dyn RenderingContext>,
    delegate: Rc<Delegate>,
    width: u32,
    height: u32,
}

#[derive(Debug)]
pub struct PageReport {
    pub title: Option<String>,
    pub status_text: Option<String>,
    pub status: LoadStatus,
    pub frames: u32,
}

impl HeadlessPage {
    pub fn report(&self) -> PageReport {
        PageReport {
            title: self.delegate.title.borrow().clone(),
            status_text: self.delegate.status_text.borrow().clone(),
            status: self.delegate.status.get(),
            frames: self.delegate.frames.get(),
        }
    }

    fn read_richness(&self, rect: DeviceIntRect) -> Option<(image::RgbaImage, u64)> {
        let img = self.context.read_to_image(rect)?;
        let raw = img.as_raw();
        let mut count = 0u64;
        let mut i = 0;
        while i + 3 < raw.len() {
            if raw[i] != 255 || raw[i + 1] != 255 || raw[i + 2] != 255 {
                count += 1;
            }
            i += 40; // sample every 10th pixel
        }
        Some((img, count))
    }

    /// Paint, wait for fresh frames, keep the richest one. Saves progress
    /// incrementally to `save_path` (atomic rename) so a mid-settle crash
    /// still leaves a usable render. Always does a final unconditional read:
    /// some pages report no new frames while holding a complete buffer.
    pub fn settle_best_frame(
        &self,
        session: &HeadlessSession,
        settle: Duration,
        save_path: Option<&std::path::Path>,
    ) -> (Option<image::RgbaImage>, u64) {
        let rect = DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(0, 0),
            DeviceIntSize::new(self.width as i32, self.height as i32),
        );
        let mut best: Option<image::RgbaImage> = None;
        let mut best_nonwhite = 0u64;
        let mut last_frames = self.delegate.frames.get();
        let mut last_log = Instant::now();
        let start = Instant::now();
        let save_best = |img: &image::RgbaImage| {
            if let Some(path) = save_path {
                // Temp name must keep a valid image extension.
                let part = std::path::PathBuf::from(format!("{}.part.png", path.display()));
                match img.save(&part) {
                    Ok(()) => match std::fs::rename(&part, path) {
                        Ok(()) => eprintln!("spike progress: saved frame {}x{}", img.width(), img.height()),
                        Err(e) => eprintln!("spike progress: rename failed: {e}"),
                    },
                    Err(e) => eprintln!("spike progress: save failed: {e}"),
                }
            }
        };
        self.webview.paint();
        while start.elapsed() < settle {
            session.pump_once(Duration::from_millis(100));
            if last_log.elapsed() > Duration::from_secs(2) {
                eprintln!(
                    "spike progress: frames={} best_nonwhite={} elapsed_ms={}",
                    self.delegate.frames.get(),
                    best_nonwhite,
                    start.elapsed().as_millis()
                );
                last_log = Instant::now();
            }
            if self.delegate.frames.get() != last_frames {
                last_frames = self.delegate.frames.get();
                if let Some((img, count)) = self.read_richness(rect) {
                    if count >= best_nonwhite {
                        best_nonwhite = count;
                        save_best(&img);
                        best = Some(img);
                    }
                }
                self.webview.paint();
            }
            if self.delegate.frames.get() > 0
                && last_frames == self.delegate.frames.get()
                && start.elapsed() > Duration::from_millis(1500)
            {
                break;
            }
        }
        // Unconditional final read: never return None when pixels exist.
        if let Some((img, count)) = self.read_richness(rect) {
            if best.is_none() || count > best_nonwhite {
                eprintln!("spike progress: final read nonwhite={count}");
                best_nonwhite = count;
                save_best(&img);
                best = Some(img);
            }
        }
        eprintln!(
            "spike progress: settle done frames={} best_nonwhite={}",
            self.delegate.frames.get(),
            best_nonwhite
        );
        self.context.present();
        (best, best_nonwhite)
    }
}
