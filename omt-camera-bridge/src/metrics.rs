//! Per-frame timing telemetry, emitted via `tracing` - kept separate from
//! the video pipeline logic in `main`.

use std::time::{Duration, Instant};

/// Times a closure, returning its result and elapsed duration - decoupled
/// from what it's timing.
fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let result = f();
    (result, start.elapsed())
}

/// Accumulates per-frame conversion/send timings and periodically reports
/// their averages. `main` only calls `time_conversion`/`time_send`/
/// `record_frame`/`maybe_report` and never touches a `Duration` directly.
pub struct Metrics {
    conversion: Duration,
    send: Duration,
    frames: u64,
    window_start: Instant,
    report_every: Duration,
    frame_budget: Duration,
    fps: i32,
}

impl Metrics {
    pub fn new(report_every: Duration, frame_budget: Duration, fps: i32) -> Self {
        Self {
            conversion: Duration::ZERO,
            send: Duration::ZERO,
            frames: 0,
            window_start: Instant::now(),
            report_every,
            frame_budget,
            fps,
        }
    }

    pub fn time_conversion<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let (result, elapsed) = time(f);
        self.conversion += elapsed;
        result
    }

    pub fn time_send<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let (result, elapsed) = time(f);
        self.send += elapsed;
        result
    }

    pub fn record_frame(&mut self) {
        self.frames += 1;
    }

    /// Emits the window's averages as a tracing event, then resets it -
    /// a no-op until `report_every` has elapsed.
    pub fn maybe_report(&mut self) {
        if self.window_start.elapsed() < self.report_every {
            return;
        }

        let frames = self.frames.max(1) as u32;
        let conv_avg = self.conversion / frames;
        let send_avg = self.send / frames;
        let total_avg = conv_avg + send_avg;
        let pct_of_frame_budget = total_avg.as_secs_f64() / self.frame_budget.as_secs_f64() * 100.0;
        let conv_avg_ms = conv_avg.as_secs_f64() * 1000.0;
        let send_avg_ms = send_avg.as_secs_f64() * 1000.0;
        let total_avg_ms = total_avg.as_secs_f64() * 1000.0;

        // Pre-formatted to &str - tracing-journald falls back to full-precision
        // Debug output for raw f64 fields.
        let conv_avg_ms = format!("{conv_avg_ms:.3}");
        let send_avg_ms = format!("{send_avg_ms:.3}");
        let total_avg_ms = format!("{total_avg_ms:.3}");
        let pct_of_frame_budget = format!("{pct_of_frame_budget:.2}");

        // tracing-journald upcases field names: `omt_conv_avg_ms` -> `OMT_CONV_AVG_MS`.
        tracing::info!(
            omt_conv_avg_ms = conv_avg_ms.as_str(),
            omt_send_avg_ms = send_avg_ms.as_str(),
            omt_total_avg_ms = total_avg_ms.as_str(),
            omt_frame_budget_pct = pct_of_frame_budget.as_str(),
            omt_frame_count = self.frames,
            "omt-camera-bridge: conversion avg {conv_avg_ms}ms + send (incl. VMX1 encode) avg {send_avg_ms}ms = {total_avg_ms}ms/frame ({pct_of_frame_budget}% of one frame's time budget @ {} fps) over {} frames",
            self.fps, self.frames,
        );

        *self = Self::new(self.report_every, self.frame_budget, self.fps);
    }
}
