//! M1 bench: two pages (YouTube + WhatsApp) in ONE Servo instance.
//!
//! Usage: spike_dual <timeout_secs> [out_dir]
//! Mirrors the product's single-process model: shared constellation,
//! separate rendering contexts. Prints one report block per page.
//! Exit 0 when both pages reach Complete, 2 otherwise.

use std::time::Duration;

use lattice_embed::headless::{HeadlessSession, LoadStatus};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;

const SITES: &[&str] = &[
    "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
    "https://web.whatsapp.com/",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let timeout = args
        .get(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(90);
    let out_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "/tmp/opencode".to_string());

    let start = std::time::Instant::now();
    let session = HeadlessSession::new();
    println!("user_agent={}", session.user_agent());
    let pages: Vec<_> = SITES
        .iter()
        .map(|url| session.page(url, WIDTH, HEIGHT))
        .collect();
    session.pump_until_complete(&pages, Duration::from_secs(timeout));

    let mut all_complete = true;
    for (i, (url, page)) in SITES.iter().zip(pages.iter()).enumerate() {
        let out_png = format!("{out_dir}/lattice-spike-dual-{i}.png");
        let out_path = std::path::PathBuf::from(&out_png);
        let (best, best_nonwhite) =
            page.settle_best_frame(&session, Duration::from_secs(12), Some(&out_path));
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
        println!("url={url}");
        println!("title={}", report.title.as_deref().unwrap_or(""));
        println!("status={:?}", report.status);
        println!("frames={}", report.frames);
        println!("nonwhite_sample10={best_nonwhite}");
        println!("pixels={saved} {dims:?} -> {out_png}");
        all_complete &= report.status == LoadStatus::Complete;
    }
    println!("elapsed_ms={}", start.elapsed().as_millis());

    std::process::exit(if all_complete { 0 } else { 2 });
}
