//! M0 compat spike: windowless Servo page load + pixel readback.
//!
//! Usage: spike_load <url> [timeout_secs] [out_png]
//! Exit 0 on LoadStatus::Complete, 2 on timeout (still prints what happened).
//!
//! This exercises the exact MVP rendering path from the spec: Servo renders
//! offscreen into `SoftwareRenderingContext`, Lattice reads back RGBA pixels
//! (later uploaded to a GPUI texture). No window, no GPUI needed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::{
    EventLoopWaker, LoadStatus, RenderingContext, ServoBuilder, SoftwareRenderingContext, WebView,
    WebViewBuilder, WebViewDelegate,
};
use webrender_api::units::{DeviceIntPoint, DeviceIntRect, DeviceIntSize};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url_str = args.get(1).map(|s| s.as_str()).unwrap_or("https://example.com/");
    let timeout = args
        .get(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30);
    let out_png = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "/tmp/opencode/lattice-spike.png".to_string());

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("rustls provider");

    let url = url::Url::parse(url_str).expect("valid URL");
    let (tx, rx) = std::sync::mpsc::channel::<()>();

    let servo = ServoBuilder::default()
        .event_loop_waker(Box::new(Waker { tx }))
        .build();
    servo.setup_logging();

    let context: Rc<dyn RenderingContext> = Rc::new(
        SoftwareRenderingContext::new(PhysicalSize::new(WIDTH, HEIGHT))
            .expect("software rendering context"),
    );

    let delegate = Delegate::new();
    let webview = WebViewBuilder::new(&servo, context.clone())
        .url(url.clone())
        .delegate(delegate.clone())
        .build();
    webview.show();

    let start = Instant::now();
    let limit = Duration::from_secs(timeout);
    loop {
        servo.spin_event_loop();
        if delegate.status.get() == LoadStatus::Complete {
            break;
        }
        if start.elapsed() >= limit {
            break;
        }
        let _ = rx.recv_timeout(Duration::from_millis(50));
        servo.spin_event_loop();
    }

    // Settle: paint, wait for fresh frames, and keep the richest frame.
    // Text often arrives in later frames (async font load -> reflow).
    let rect = DeviceIntRect::from_origin_and_size(
        DeviceIntPoint::new(0, 0),
        DeviceIntSize::new(WIDTH as i32, HEIGHT as i32),
    );
    let mut best_nonwhite = 0u64;
    let mut best: Option<image::RgbaImage> = None;
    let mut last_frames = delegate.frames.get();
    let settle_start = Instant::now();
    webview.paint();
    while settle_start.elapsed() < Duration::from_secs(12) {
        servo.spin_event_loop();
        let _ = rx.recv_timeout(Duration::from_millis(100));
        servo.spin_event_loop();
        if delegate.frames.get() != last_frames {
            last_frames = delegate.frames.get();
            if let Some(img) = context.read_to_image(rect) {
                let raw = img.as_raw();
                let mut count = 0u64;
                let mut i = 0;
                while i + 3 < raw.len() {
                    if raw[i] != 255 || raw[i + 1] != 255 || raw[i + 2] != 255 {
                        count += 1;
                    }
                    i += 40;
                }
                if count >= best_nonwhite {
                    best_nonwhite = count;
                    best = Some(img);
                }
            }
            webview.paint();
        }
        // Quiescent: no new frames for a while after seeing content frames.
        if delegate.frames.get() > 0
            && last_frames == delegate.frames.get()
            && settle_start.elapsed() > Duration::from_millis(1500)
        {
            // One last read in case the final frame was never sampled.
            if best.is_none() {
                if let Some(img) = context.read_to_image(rect) {
                    let raw = img.as_raw();
                    let mut count = 0u64;
                    let mut i = 0;
                    while i + 3 < raw.len() {
                        if raw[i] != 255 || raw[i + 1] != 255 || raw[i + 2] != 255 {
                            count += 1;
                        }
                        i += 40;
                    }
                    best_nonwhite = count;
                    best = Some(img);
                }
            }
            break;
        }
    }
    context.present();
    let (saved, dims) = match &best {
        Some(img) => {
            let dims = (img.width(), img.height());
            match img.save(&out_png) {
                Ok(()) => (true, dims),
                Err(e) => {
                    eprintln!("png save failed: {e}");
                    (false, dims)
                }
            }
        }
        None => (false, (0, 0)),
    };

    println!("url={url_str}");
    println!("title={:?}", delegate.title.borrow());
    println!("status_text={:?}", delegate.status_text.borrow());
    println!("status={:?}", delegate.status.get());
    println!("frames={}", delegate.frames.get());
    println!("nonwhite_sample10={best_nonwhite}");
    println!("elapsed_ms={}", start.elapsed().as_millis());
    println!("pixels={saved} {dims:?} -> {out_png}");

    if delegate.status.get() == LoadStatus::Complete {
        std::process::exit(0);
    } else {
        std::process::exit(2);
    }
}
