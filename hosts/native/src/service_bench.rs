//! `bench`, a synthetic service of `/dev/gex-services` (services.rs) for
//! measuring the channel, offered with `GEX_SERVICES_BENCH=1`; the page's
//! is test/darwin/services-bench's, and fixtures/darwin/servicebench.c the
//! program that measures. Its messages, version 1:
//!
//! - `{"op":"pull","seq","size","count","rate"}`: `count` frames
//!   `{"op":"frame","n"}`, each with `size` bytes of data (byte i of frame
//!   n is `(n + i) & 255`), `rate` a second (0: as fast as the program takes
//!   them), never more than two frames ahead of the program; then
//!   `{"seq","op":"pulled"}`.
//! - `{"op":"ping","seq","t"}`: `{"seq","op":"pong","t"}` at once.
//! - `{"op":"ticks","seq","count","interval"}`: `count` events
//!   `{"op":"tick","n"}`, `interval` ms apart; then `{"seq","op":"ticked"}`.

use crate::services::{Json, Link, Service, Session};
use std::time::{Duration, Instant};

pub struct Bench;

impl Service for Bench {
    fn name(&self) -> &'static str {
        "bench"
    }
    fn version(&self) -> u32 {
        1
    }
    fn open(&self, link: Link, _version: u32) -> Box<dyn Session> {
        Box::new(BenchSession { link })
    }
}

struct BenchSession {
    link: Link,
}

/// Pad a message to `want` bytes of JSON (a frame of `want + 8`).
fn padded(body: &str, want: usize) -> String {
    let base = format!("{{{body},\"p\":\"\"}}");
    let pad = want.saturating_sub(base.len());
    format!("{{{body},\"p\":\"{}\"}}", "x".repeat(pad))
}

impl Session for BenchSession {
    fn message(&mut self, m: &Json, _data: &[u8]) {
        let seq = m.num("seq").unwrap_or(0.0) as i64;
        let link = self.link.clone();
        match m.str("op").unwrap_or("") {
            "ping" => {
                let t = m.num("t").unwrap_or(0.0);
                link.send(&padded(&format!("\"seq\":{seq},\"op\":\"pong\",\"t\":{t}"), 56), &[]);
            }
            "pull" => {
                let size = (m.num("size").unwrap_or(0.0) as usize).min(64 << 20);
                let count = m.num("count").unwrap_or(0.0) as u64;
                let rate = m.num("rate").unwrap_or(0.0);
                std::thread::spawn(move || {
                    let start = Instant::now();
                    let mut data = vec![0u8; size];
                    for n in 0..count {
                        if rate > 0.0 {
                            let due = start + Duration::from_secs_f64(n as f64 / rate);
                            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                                std::thread::sleep(wait);
                            }
                        }
                        // Two frames ahead at most: a slow program does not
                        // make this buffer without bound.
                        while link.buffered().is_some_and(|b| b > 2 * size + 64) {
                            std::thread::sleep(Duration::from_micros(200));
                        }
                        for (i, b) in data.iter_mut().enumerate() {
                            *b = (n as usize + i) as u8;
                        }
                        if !link.send(&format!("{{\"op\":\"frame\",\"n\":{n}}}"), &data) {
                            return;
                        }
                    }
                    link.send(&format!("{{\"seq\":{seq},\"op\":\"pulled\"}}"), &[]);
                });
            }
            "ticks" => {
                let count = m.num("count").unwrap_or(0.0) as u64;
                let interval = m.num("interval").unwrap_or(1.0).max(0.0);
                std::thread::spawn(move || {
                    let start = Instant::now();
                    for n in 0..count {
                        let due = start + Duration::from_secs_f64(n as f64 * interval / 1000.0);
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                        if !link.send(&padded(&format!("\"op\":\"tick\",\"n\":{n}"), 56), &[]) {
                            return;
                        }
                    }
                    link.send(&format!("{{\"seq\":{seq},\"op\":\"ticked\"}}"), &[]);
                });
            }
            _ => {}
        }
    }
}
