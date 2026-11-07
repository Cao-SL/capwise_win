//! `CAP_PERF_MARKS=1`: log named startup checkpoints with the time since
//! `main` was entered, so `scripts/bench_perf.py` can split launch latency
//! into process load, AppKit/gpui launch and our own setup. Off by default;
//! when off a mark is one relaxed atomic load.

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static MAIN_ENTERED: OnceLock<Instant> = OnceLock::new();

pub fn start() {
    MAIN_ENTERED.get_or_init(Instant::now);
    ENABLED.store(
        std::env::var("CAP_PERF_MARKS").is_ok_and(|value| value == "1"),
        Ordering::Relaxed,
    );
}

pub fn mark(name: &'static str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let since_main_ms = MAIN_ENTERED
        .get()
        .map(|entered| entered.elapsed().as_secs_f64() * 1000.0);
    tracing::info!(name, since_main_ms, "perf mark");
}
