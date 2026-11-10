//! M0 compat spike: windowless Servo page load + pixel readback.
//!
//! Usage: spike_load <url> [timeout_secs] [out_png]
//! Exit 0 on LoadStatus::Complete, 2 on timeout (still prints what happened).
//!
//! Thin wrapper over `lattice_embed::headless`: Servo renders offscreen into
//! `SoftwareRenderingContext`, Lattice reads back RGBA pixels (later uploaded
//! to a GPUI texture). No window, no GPUI needed.

use std::time::Duration;

use lattice_embed::headless::HeadlessSession;

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

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

    let start = std::time::Instant::now();
    let session = HeadlessSession::new();
    println!("user_agent={}", session.user_agent());
    let page = session.page(url_str, WIDTH, HEIGHT);
    session.pump_until_complete(std::slice::from_ref(&page), Duration::from_secs(timeout));
    let (best, best_nonwhite) = page.settle_best_frame(&session, Duration::from_secs(12));
    let report = page.report();

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
    println!("title={}", report.title.as_deref().unwrap_or(""));
    println!("status_text={:?}", report.status_text);
    println!("status={:?}", report.status);
    println!("frames={}", report.frames);
    println!("nonwhite_sample10={best_nonwhite}");
    println!("elapsed_ms={}", start.elapsed().as_millis());
    println!("pixels={saved} {dims:?} -> {out_png}");

    if report.status == lattice_embed::headless::LoadStatus::Complete {
        std::process::exit(0);
    } else {
        std::process::exit(2);
    }
}
