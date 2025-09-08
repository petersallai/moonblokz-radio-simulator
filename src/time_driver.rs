use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration as StdDuration;
use embassy_time_driver::{Driver, TICK_HZ};
use std::sync::OnceLock;
use std::task::Waker;
use std::thread;

// Global speed percentage (100 = 1.0x)
static SPEED_PERCENT: AtomicU32 = AtomicU32::new(100);

pub fn set_speed_percent(percent: u32) {
    SPEED_PERCENT.store(percent.clamp(1, 10_000), Ordering::Relaxed);
}

pub fn get_speed_percent() -> u32 {
    SPEED_PERCENT.load(Ordering::Relaxed)
}

fn scale_real_delta_ticks(sim_ticks: u64) -> StdDuration {
    if sim_ticks == 0 {
        return StdDuration::from_nanos(0);
    }
    let speed = SPEED_PERCENT.load(Ordering::Relaxed) as u128;
    let ticks = sim_ticks as u128;
    // sim_ns = ticks * 1e9 / TICK_HZ
    let sim_ns = ticks.saturating_mul(1_000_000_000u128).saturating_div(TICK_HZ as u128);
    // real_ns = sim_ns * 100 / speed_percent
    let mut real_ns = sim_ns.saturating_mul(100).saturating_div(speed.max(1));
    if sim_ns > 0 {
        real_ns = real_ns.max(1);
    }
    StdDuration::from_nanos(real_ns as u64)
}

struct StdScaledDriver {}

static START_INSTANT: OnceLock<std::time::Instant> = OnceLock::new();

fn now_ticks_global() -> u64 {
    let start = START_INSTANT.get_or_init(std::time::Instant::now);
    let elapsed = start.elapsed();
    // Simulated ticks = real elapsed (in ticks) scaled up by speed
    let speed = SPEED_PERCENT.load(Ordering::Relaxed) as u128;
    let ns: u128 = elapsed.as_nanos();
    let real_ticks = ns.saturating_mul(TICK_HZ as u128).saturating_div(1_000_000_000u128);
    let sim_ticks = real_ticks.saturating_mul(speed).saturating_div(100);
    sim_ticks as u64
}

impl Driver for StdScaledDriver {
    fn now(&self) -> u64 {
        now_ticks_global()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        let now = now_ticks_global();
        if at <= now {
            waker.wake_by_ref();
            return;
        }

        let w = waker.clone();
        thread::spawn(move || {
            loop {
                let now2 = now_ticks_global();
                if at <= now2 {
                    w.wake_by_ref();
                    break;
                }
                let remaining = at - now2;
                // Sleep a chunk of the remaining, scaled to real time. Cap to ~1s real to react to speed changes.
                let chunk_ticks = remaining.min(TICK_HZ as u64);
                let d = scale_real_delta_ticks(chunk_ticks);
                if d.is_zero() {
                    // Avoid busy loop if extremely short; yield briefly
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(d);
                }
            }
        });
    }
}

//embassy_time_driver::time_driver_impl!(static DRIVER_INST: StdScaledDriver = StdScaledDriver {});

// Public API for UI to get/set speed in percent
pub fn get_ui_speed_percent() -> u32 {
    get_speed_percent()
}
pub fn set_ui_speed_percent(v: u32) {
    set_speed_percent(v)
}
