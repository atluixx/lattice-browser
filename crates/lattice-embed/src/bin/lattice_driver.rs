//! lattice-driver: persistent single-tab Servo renderer.
//!
//! One process per tab. Speaks JSON lines on stdio:
//!   in:  {"cmd":"navigate","url":"..."} | {"cmd":"back"} |
//!        {"cmd":"forward"} | {"cmd":"reload"} |
//!        {"cmd":"scroll","dx":f64,"dy":f64,"x":f32,"y":f32} |
//!        {"cmd":"click","x":f32,"y":f32} |
//!        {"cmd":"resize","w":u32,"h":u32} | {"cmd":"quit"}
//!   out: {"ev":"ready"} | {"ev":"title","title":..} |
//!        {"ev":"status","text":..} | {"ev":"load","state":".."} |
//!        {"ev":"frame","path":..,"w":..,"h":..} |
//!        {"ev":"favicon","path":..} | {"ev":"bye"}
//!
//! Frames stream as numbered PNGs in the out dir (odd traffic is fine:
//! the shell polls mtimes). The process ends on `quit`, stdin EOF, or
//! fatal error. No orderly Servo shutdown exists in 0.1.4; teardown may
//! segfault after the last line is printed, which the shell tolerates.

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use lattice_embed::headless::{HeadlessSession, LoadStatus};
use serde_json::{Value, json};

fn emit(v: Value) {
    println!("{}", v);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let outdir = args.get(1).cloned().unwrap_or_else(|| "/tmp/opencode".to_string());
    let (mut width, mut height) = (1280u32, 800u32);
    if let (Some(w), Some(h)) = (args.get(2), args.get(3)) {
        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
            width = w;
            height = h;
        }
    }
    let start_url = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "about:blank".to_string());

    let session = HeadlessSession::new();
    let page = session.page(&start_url, width, height);
    emit(json!({"ev":"ready","w":width,"h":height}));

    let (tx, rx): (std::sync::mpsc::Sender<String>, Receiver<String>) = channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in BufReader::new(stdin.lock()).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut last_frames = 0u32;
    let mut last_emit = Instant::now();
    let mut frame_seq = 0u64;
    let mut last_title: Option<String> = None;
    let mut last_status = LoadStatus::Started;
    let mut favicon_done = false;
    let mut last_paint = Instant::now();

    loop {
        session.spin();
        // Drain commands.
        let mut quit = false;
        while let Ok(line) = rx.try_recv() {
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v.get("cmd").and_then(|c| c.as_str()) {
                Some("quit") => {
                    quit = true;
                    break;
                }
                Some("navigate") => {
                    if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                        page.load(url);
                    }
                }
                Some("back") => page.go_back(),
                Some("forward") => page.go_forward(),
                Some("reload") => page.reload(),
                Some("resize") => {
                    let w = v.get("w").and_then(|x| x.as_u64()).unwrap_or(1280) as u32;
                    let h = v.get("h").and_then(|x| x.as_u64()).unwrap_or(800) as u32;
                    width = w;
                    height = h;
                    page.resize(w, h);
                    page.paint();
                }
                Some("scroll") => {
                    let dx = v.get("dx").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    let dy = v.get("dy").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    page.scroll_by(dx, dy, x, y);
                }
                Some("click") => {
                    let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    page.click_at(x, y);
                }
                _ => {}
            }
        }
        if quit {
            break;
        }
        session.spin();

        // State changes.
        let report = page.report();
        if report.title != last_title {
            last_title = report.title.clone();
            emit(json!({"ev":"title","title":report.title}));
        }
        if report.status != last_status {
            last_status = report.status;
            emit(json!({"ev":"load","state":format!("{:?}",report.status)}));
        }
        if !favicon_done {
            if let Some(icon) = page.favicon_image() {
                let path = format!("{outdir}/favicon.png");
                if icon.save(&path).is_ok() {
                    emit(json!({"ev":"favicon","path":path}));
                    favicon_done = true;
                }
            }
        }

        // Frames: paint + readback on change, throttled; repaint
        // periodically so animations keep flowing.
        let frames = page.frames();
        let settled_enough = last_emit.elapsed() >= Duration::from_millis(100);
        if frames != last_frames {
            last_frames = frames;
            page.paint();
        } else if last_paint.elapsed() >= Duration::from_millis(500) {
            last_paint = Instant::now();
            page.paint();
        }
        if settled_enough {
            if let Some((img, _)) = read_frame(&page) {
                frame_seq += 1;
                let path = format!("{outdir}/frame-{frame_seq}.png");
                if img.save(&path).is_ok() {
                    if frame_seq > 2 {
                        let _ = std::fs::remove_file(format!(
                            "{outdir}/frame-{}.png",
                            frame_seq - 2
                        ));
                    }
                    emit(json!({"ev":"frame","path":path,"w":width,"h":height}));
                    last_emit = Instant::now();
                }
            }
        }
        session.wait_wake(Duration::from_millis(30));
    }
    emit(json!({"ev":"bye"}));
}

fn read_frame(
    page: &lattice_embed::headless::HeadlessPage,
) -> Option<(image::RgbaImage, u64)> {
    // Reuse the richness sampler through a paint+read cycle is handled by
    // the caller; here just read the current backbuffer.
    let img = page.read_image()?;
    let raw = img.as_raw();
    let mut count = 0u64;
    let mut i = 0;
    while i + 3 < raw.len() {
        if raw[i] != 255 || raw[i + 1] != 255 || raw[i + 2] != 255 {
            count += 1;
        }
        i += 40;
    }
    Some((img, count))
}
