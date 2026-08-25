//! How evenly the engine is presenting, as opposed to how often.
//!
//! **A frame rate cannot tell you whether something stutters, and this project
//! has the scars to prove it.** AGENTS.md's rule about never quoting present
//! counts as a frame rate is about the same confusion one level down: a count
//! over a window is a mean, and a mean is exactly the statistic that hides
//! judder. A client presenting 120 frames a second in evenly spaced 8.3 ms
//! steps and one presenting 120 frames as sixty pairs 16 ms apart have the same
//! count and look completely different.
//!
//! Reported as "why do I feel like roblox is laggy or stuttering even though
//! its at 120 fps", which is a question the instrumentation could not answer
//! at all -- nothing recorded when a frame went out, only that one did.
//!
//! So this keeps the interval between consecutive presents, and reports the
//! distribution rather than the average. What matters for the feel of it is
//! the tail: p50 is the frame rate everyone quotes, and p99 is the one being
//! complained about.
//!
//! Deliberately cheap, because it runs on the engine's render thread inside
//! `vkQueuePresentKHR` and instrumentation that perturbs the thing it measures
//! is the broken instrument AGENTS.md keeps warning about. One monotonic clock
//! read and two relaxed atomic stores per frame; no allocation, no lock, and
//! nothing that can block the presenting thread.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// A few seconds at any plausible frame rate. Power of two so the index wraps
/// with a mask rather than a division.
const RING: usize = 1024;

static INTERVALS_US: [AtomicU32; RING] = [const { AtomicU32::new(0) }; RING];
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Nanoseconds, from the same monotonic base as every other reading here.
static LAST_NS: AtomicU64 = AtomicU64::new(0);

fn now_ns() -> u64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(std::time::Instant::now);
    base.elapsed().as_nanos() as u64
}

/// Called once per `vkQueuePresentKHR`, before the present is forwarded.
pub fn record_present() {
    let now = now_ns();
    let last = LAST_NS.swap(now, Ordering::Relaxed);
    if last == 0 || now <= last {
        return;
    }
    // Saturating rather than wrapping: a gap of more than about an hour is a
    // suspended session, and recording it as a small number would be a lie in
    // the direction that hides a stall.
    let us = ((now - last) / 1_000).min(u32::MAX as u64) as u32;
    let slot = HEAD.fetch_add(1, Ordering::Relaxed) & (RING - 1);
    INTERVALS_US[slot].store(us, Ordering::Relaxed);
}

/// The distribution of the last few seconds of frame intervals.
///
/// `None` until enough frames have gone out to say anything: a percentile over
/// three samples is not a percentile, and reporting one would invite exactly
/// the over-reading of a thin measurement this file exists to prevent.
pub fn summary() -> Option<String> {
    let seen = HEAD.load(Ordering::Relaxed);
    if seen < 32 {
        return None;
    }
    let n = seen.min(RING);
    let mut v: Vec<u32> = (0..n).map(|i| INTERVALS_US[i].load(Ordering::Relaxed)).collect();
    v.retain(|&x| x > 0);
    if v.len() < 32 {
        return None;
    }
    v.sort_unstable();
    let at = |q: f64| -> f64 {
        let i = ((v.len() - 1) as f64 * q).round() as usize;
        v[i] as f64 / 1000.0
    };
    let p50 = at(0.50);
    // The rate everyone quotes, derived from the median interval rather than
    // from a count over a window, so a stall cannot average itself away.
    let fps = if p50 > 0.0 { 1000.0 / p50 } else { 0.0 };
    Some(format!(
        "frames n={} p50={:.1}ms p95={:.1}ms p99={:.1}ms max={:.1}ms median_fps={:.0}",
        v.len(),
        p50,
        at(0.95),
        at(0.99),
        v[v.len() - 1] as f64 / 1000.0,
        fps
    ))
}
