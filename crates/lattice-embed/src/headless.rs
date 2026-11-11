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
    EventLoopWaker, InputEvent, MouseButton, MouseButtonAction, MouseButtonEvent,
    RenderingContext, Scroll, Servo, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate, WebViewPoint, WebViewVector, WheelDelta, WheelEvent,
    WheelMode,
};
use webrender_api::units::{
    DeviceIntPoint, DeviceIntRect, DeviceIntSize, DevicePoint, DeviceVector2D,
};

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

    /// One spin pass, for drivers that multiplex stdin with the event loop.
    pub fn spin(&self) {
        self.servo.spin_event_loop();
    }

    /// Poll the waker channel with a timeout (lets a driver loop watch
    /// stdin and Servo wakeups together).
    pub fn wait_wake(&self, wait: Duration) {
        let _ = self.rx.recv_timeout(wait);
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

    pub fn frames(&self) -> u32 {
        self.delegate.frames.get()
    }

    pub fn load(&self, url: &str) {
        if let Ok(url) = url::Url::parse(url) {
            self.webview.load(url);
        }
    }

    pub fn reload(&self) {
        self.webview.reload();
    }

    pub fn go_back(&self) {
        self.webview.go_back(1);
    }

    pub fn go_forward(&self) {
        self.webview.go_forward(1);
    }

    pub fn resize(&self, width: u32, height: u32) {
        let size = PhysicalSize::new(width, height);
        self.context.resize(size);
        self.webview.resize(size);
    }

    fn device_point(&self, x: f32, y: f32) -> WebViewPoint {
        WebViewPoint::Device(DevicePoint::new(x, y))
    }

    /// Scroll by pixel deltas at the given viewport point.
    pub fn scroll_by(&self, dx: f64, dy: f64, x: f32, y: f32) {
        let point = self.device_point(x, y);
        let delta = WebViewVector::Device(DeviceVector2D::new(dx as f32, dy as f32));
        self.webview
            .notify_scroll_event(Scroll::Delta(delta), point);
        self.webview.paint();
    }

    /// Click (down + up) with the left button at the given point.
    pub fn click_at(&self, x: f32, y: f32) {
        let point = self.device_point(x, y);
        for action in [MouseButtonAction::Down, MouseButtonAction::Up] {
            self.webview.notify_input_event(InputEvent::MouseButton(
                MouseButtonEvent::new(action, MouseButton::Left, point),
            ));
        }
        self.webview.paint();
    }

    /// Forward a mouse-wheel event (used for trackpad-style scrolling too).
    pub fn wheel(&self, dx: f64, dy: f64, x: f32, y: f32) {
        let delta = WheelDelta {
            x: dx,
            y: dy,
            z: 0.0,
            mode: WheelMode::DeltaPixel,
        };
        let point = self.device_point(x, y);
        self.webview
            .notify_input_event(InputEvent::Wheel(WheelEvent::new(delta, point)));
        self.webview.paint();
    }

    pub fn paint(&self) {
        self.webview.paint();
    }

    pub fn read_image(&self) -> Option<image::RgbaImage> {
        let rect = DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(0, 0),
            DeviceIntSize::new(self.width as i32, self.height as i32),
        );
        self.context.read_to_image(rect)
    }

    pub fn present(&self) {
        self.context.present();
    }

    /// Current favicon pixels, if the page provided any, converted to RGBA.
    pub fn favicon_image(&self) -> Option<image::RgbaImage> {
        let icon = self.webview.favicon()?;
        let (w, h, format, raw) = (icon.width, icon.height, icon.format, icon.data().to_vec());
        drop(icon);
        match format {
            servo::PixelFormat::RGBA8 => image::RgbaImage::from_raw(w, h, raw),
            servo::PixelFormat::BGRA8 => {
                let mut px = raw;
                for chunk in px.chunks_exact_mut(4) {
                    chunk.swap(0, 2);
                }
                image::RgbaImage::from_raw(w, h, px)
            }
            servo::PixelFormat::RGB8 => {
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for rgb in raw.chunks_exact(3) {
                    px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
                image::RgbaImage::from_raw(w, h, px)
            }
            _ => None,
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
