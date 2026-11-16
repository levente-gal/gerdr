//! Counts of gerdr's drawing, to tell in numbers whether a change made it
//! cheaper: with `GERDR_BENCH=<file>`, one JSON line a second goes there.
//! `frames` were drawn, `drawn` of them changed pixels (`damage` of them,
//! in all); `view_us`, `elements_us` and `render_us` add up the time spent
//! building the chrome's view, its elements, and drawing and handing the
//! frame over; `max_us` is the slowest frame; `chrome` the most chrome
//! elements in one frame.

use serde_json::json;
use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

/// What one frame took.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub view: Duration,
    pub elements: Duration,
    /// From the start of the frame to its hand over.
    pub total: Duration,
    /// Pixels drawn again.
    pub damage: u64,
    /// Chrome elements drawn.
    pub chrome: usize,
}

/// The second being counted, and where it goes.
pub struct Bench {
    out: File,
    started: Instant,
    since: Instant,
    count: Count,
}

/// What the frames of one second added up to.
#[derive(Default)]
struct Count {
    frames: u64,
    drawn: u64,
    damage: u64,
    view: Duration,
    elements: Duration,
    render: Duration,
    max: Duration,
    chrome: usize,
}

impl Bench {
    /// Counting, if `GERDR_BENCH` names a file that can be written.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("GERDR_BENCH")?;
        match File::create(&path) {
            Ok(out) => Some(Self::new(out, Instant::now())),
            Err(err) => {
                let path = path.to_string_lossy();
                tracing::warn!("GERDR_BENCH: can't write {path}: {err}");
                None
            }
        }
    }

    fn new(out: File, now: Instant) -> Self {
        Self {
            out,
            started: now,
            since: now,
            count: Count::default(),
        }
    }

    /// Count `frame`, finished `now`; a second after the last line, write
    /// the next.
    pub fn frame(&mut self, frame: Frame, now: Instant) {
        let c = &mut self.count;
        c.frames += 1;
        c.drawn += u64::from(frame.damage > 0);
        c.damage += frame.damage;
        c.view += frame.view;
        c.elements += frame.elements;
        c.render += frame.total.saturating_sub(frame.view + frame.elements);
        c.max = c.max.max(frame.total);
        c.chrome = c.chrome.max(frame.chrome);
        if now.duration_since(self.since) < Duration::from_secs(1) {
            return;
        }
        let line = self.line(now);
        if let Err(err) = writeln!(self.out, "{line}") {
            tracing::warn!("GERDR_BENCH: {err}");
        }
        self.count = Count::default();
        self.since = now;
    }

    fn line(&self, now: Instant) -> serde_json::Value {
        let us = |d: Duration| d.as_micros() as u64;
        let c = &self.count;
        json!({
            "t": now.duration_since(self.started).as_secs_f64(),
            "frames": c.frames,
            "drawn": c.drawn,
            "damage": c.damage,
            "view_us": us(c.view),
            "elements_us": us(c.elements),
            "render_us": us(c.render),
            "max_us": us(c.max),
            "chrome": c.chrome,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_of_frames_makes_one_line() {
        let start = Instant::now();
        let mut bench = Bench::new(File::open("/dev/null").unwrap(), start);
        let ms = Duration::from_millis;
        let frame = |damage| Frame {
            view: ms(1),
            elements: ms(2),
            total: ms(5),
            damage,
            chrome: 7,
        };
        bench.frame(frame(100), start + ms(16));
        bench.frame(frame(0), start + ms(32));
        let line = bench.line(start + ms(1000));
        assert_eq!(line["frames"], 2);
        assert_eq!(line["drawn"], 1);
        assert_eq!(line["damage"], 100);
        assert_eq!(line["view_us"], 2000);
        assert_eq!(line["render_us"], 4000);
        assert_eq!(line["max_us"], 5000);
        assert_eq!(line["chrome"], 7);
        // The second over: counted again from nothing.
        bench.frame(frame(1), start + ms(1000));
        assert_eq!(bench.line(start + ms(1001))["frames"], 0);
    }
}
