//! Servo driver thread for the M1 composite.
//!
//! Runs one `HeadlessSession` off the UI thread (Servo is `!Send`) and ships
//! settled RGBA frames to the GPUI thread over a channel. The thread parks
//! instead of tearing Servo down: 0.1.4 has no orderly shutdown API and
//! process-exit teardown segfaults (known M0 finding).

use std::sync::mpsc::Sender;
use std::time::Duration;

pub struct FrameMsg {
    pub title: Option<String>,
    pub status: String,
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn spawn_servo(url: String, tx: Sender<FrameMsg>) {
    std::thread::spawn(move || {
        let session = lattice_embed::headless::HeadlessSession::new();
        let page = session.page(&url, 1280, 800);
        session.pump_until_complete(std::slice::from_ref(&page), Duration::from_secs(60));
        // Keep refreshing: animated pages produce new frames; the shell
        // always shows the richest recent one.
        loop {
            let (best, _) = page.settle_best_frame(&session, Duration::from_secs(4), None);
            let report = page.report();
            if let Some(img) = best {
                let (w, h) = (img.width(), img.height());
                let msg = FrameMsg {
                    title: report.title,
                    status: format!("{:?} frames={}", report.status, report.frames),
                    pixels: img.into_raw(),
                    width: w,
                    height: h,
                };
                if tx.send(msg).is_err() {
                    break; // UI gone.
                }
            } else if tx
                .send(FrameMsg {
                    title: report.title,
                    status: format!("{:?} frames={} (no pixels)", report.status, report.frames),
                    pixels: Vec::new(),
                    width: 0,
                    height: 0,
                })
                .is_err()
            {
                break;
            }
        }
    });
}
