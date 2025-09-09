use core::task::Waker;
use embassy_time_driver::{Driver, TICK_HZ, time_driver_impl};
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant as StdInstant};

// Fixed-point Q32.32 for speed scaling. 1.0 == 1<<32
const ONE_Q32: u64 = 1u64 << 32;

#[derive(Debug)]
struct ScaledClock {
    origin_real: StdInstant,   // host reference time
    origin_virtual_ticks: u64, // in embassy ticks
    scale_q32: u64,            // e.g., 0.5 = 0.5 * ONE_Q32
}

#[derive(Default)]
struct SchedulerState {
    // Map of virtual-timestamp -> list of wakers
    queue: BTreeMap<u64, Vec<Waker>>,
    // Bumped on speed changes to help the scheduler re-evaluate waits promptly
    epoch: u64,
}

// Global singletons initialized lazily at first use
static CLOCK: OnceLock<Mutex<ScaledClock>> = OnceLock::new();
static SCHED: OnceLock<Mutex<SchedulerState>> = OnceLock::new();
static CV: OnceLock<Condvar> = OnceLock::new();
static SCHEDULER_STARTED: OnceLock<()> = OnceLock::new();

fn clock() -> &'static Mutex<ScaledClock> {
    CLOCK.get_or_init(|| {
        Mutex::new(ScaledClock {
            origin_real: StdInstant::now(),
            origin_virtual_ticks: 0,
            scale_q32: ONE_Q32,
        })
    })
}

fn sched() -> &'static Mutex<SchedulerState> {
    SCHED.get_or_init(|| Mutex::new(SchedulerState::default()))
}

fn cv() -> &'static Condvar {
    CV.get_or_init(Condvar::new)
}

fn tick_hz() -> u64 {
    TICK_HZ as u64
}

fn real_now() -> StdInstant {
    StdInstant::now()
}

fn map_real_to_virtual(r: StdInstant) -> u64 {
    let clock_lock = clock().lock().unwrap();
    let real_dt = r.saturating_duration_since(clock_lock.origin_real);
    let real_ticks = (real_dt.as_nanos() as u128 * tick_hz() as u128 / 1_000_000_000u128) as u64;
    let scaled = ((real_ticks as u128) * (clock_lock.scale_q32 as u128) / (ONE_Q32 as u128)) as u64;
    clock_lock.origin_virtual_ticks.wrapping_add(scaled)
}

fn map_virtual_to_real(v_target: u64) -> StdInstant {
    let clock_lock = clock().lock().unwrap();
    let virt_dt = v_target.wrapping_sub(clock_lock.origin_virtual_ticks);
    let real_ticks = ((virt_dt as u128) * (ONE_Q32 as u128) / (clock_lock.scale_q32 as u128)) as u64;
    let real_ns = (real_ticks as u128) * 1_000_000_000u128 / (tick_hz() as u128);
    clock_lock.origin_real + Duration::from_nanos(real_ns as u64)
}

fn ensure_scheduler_thread() {
    SCHEDULER_STARTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("embassy-time-scheduler".into())
            .spawn(move || scheduler_thread())
            .expect("failed to start embassy-time scheduler thread");
    });
}

fn scheduler_thread() {
    // Maximum slice to wait so scale changes apply promptly even if a notify is missed
    const MAX_WAIT_SLICE: Duration = Duration::from_millis(25);
    loop {
        // Determine next due time or wait for new items, without holding the queue lock
        // while accessing the clock to avoid lock-order inversion (SCHED -> CLOCK).
        // 1) Snapshot earliest deadline and epoch under the queue lock.
        let (next_at, snapshot_epoch) = loop {
            let guard = sched().lock().unwrap();
            if guard.queue.is_empty() {
                log::trace!("scheduler: queue empty; waiting for work");
                let guard = cv().wait(guard).unwrap();
                drop(guard);
                continue;
            }
            let (&next_at, _) = guard.queue.iter().next().unwrap();
            let snapshot_epoch = guard.epoch;
            drop(guard);
            log::trace!("scheduler: snapshot next_at={} epoch={}", next_at, snapshot_epoch);
            break (next_at, snapshot_epoch);
        };

        // 2) Compute real target outside of the queue lock
        let real_target = map_virtual_to_real(next_at);
        let now_r = real_now();

        if real_target > now_r {
            let mut wait_dur = real_target - now_r;
            if wait_dur > MAX_WAIT_SLICE {
                wait_dur = MAX_WAIT_SLICE;
            }
            // Reacquire the queue lock only for the timed wait; we don't access the clock while holding it.
            let mut guard = sched().lock().unwrap();
            log::trace!("scheduler: waiting up to {:?}", wait_dur);
            let (new_guard, _timeout_res) = cv().wait_timeout(guard, wait_dur).unwrap();
            guard = new_guard;
            // If epoch changed (speed slider moved) or we were notified, simply iterate again.
            if guard.epoch != snapshot_epoch {
                log::trace!("scheduler: epoch changed ({} -> {}), re-evaluating", snapshot_epoch, guard.epoch);
                drop(guard);
                continue;
            }
            drop(guard);
            continue;
        }

        // 3) Drain all due wakers. Compute virtual "now" outside queue lock to avoid CLOCK while holding SCHED.
        let now_v = map_real_to_virtual(real_now());
        let mut ready: Vec<Waker> = Vec::new();
        {
            let mut guard = sched().lock().unwrap();
            let mut to_remove = Vec::new();
            for (&ts, ws) in guard.queue.iter() {
                if ts <= now_v {
                    ready.extend(ws.iter().cloned());
                    to_remove.push(ts);
                } else {
                    break;
                }
            }
            for ts in to_remove {
                guard.queue.remove(&ts);
            }
            log::trace!("scheduler: draining due; now_v={} woke={} remaining={}",
                now_v, ready.len(), guard.queue.len());
        }

        // 4) Wake outside locks
        for w in ready.into_iter() {
            w.wake();
        }
    }
}

struct ScaledDriver;

impl Driver for ScaledDriver {
    fn now(&self) -> u64 {
    let v = map_real_to_virtual(real_now());
    log::trace!("driver.now -> {}", v);
    v
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        ensure_scheduler_thread();
        let mut guard = sched().lock().unwrap();
        guard.queue.entry(at).or_default().push(waker.clone());
    log::trace!("schedule_wake at={} queue_len={}", at, guard.queue.len());
        drop(guard);
        cv().notify_all();
    }
}

// Register as the global time driver for embassy-time
time_driver_impl!(static DRIVER: ScaledDriver = ScaledDriver);

// Public UI helpers
pub fn set_simulation_speed_percent(percent: u32) {
    let percent = percent.clamp(1, 1000);
    // Fast path: no-op if unchanged
    let current_pct = get_simulation_speed_percent();
    if current_pct == percent {
        log::debug!("Set simulation speed unchanged: {}% (no-op)", percent);
        return;
    }
    log::debug!("Set current simulation speed percent: {} (was {}%)", percent, current_pct);
    let r_now = real_now();

    // Capture old mapping parameters before changing
    let _ov_old = {
        let c = clock().lock().unwrap();
        c.origin_virtual_ticks
    };
    // Virtual 'now' under the old mapping
    let v_now_old = map_real_to_virtual(r_now);

    // Compute new scale
    let new_scale_q32 = ((percent as u128) * (ONE_Q32 as u128) / 100u128) as u64;

    // Update clock: rebase origins to keep v_now continuous, and apply new scale
    {
        let mut c = clock().lock().unwrap();
        c.origin_real = r_now;
        c.origin_virtual_ticks = v_now_old;
        c.scale_q32 = new_scale_q32;
    }

    // Keep queued wakeups' virtual deadlines unchanged. Only bump epoch so the scheduler re-evaluates waits
    // under the new real<->virtual mapping. This prevents collapsing far-future timers to 'now'.
    {
        let mut s = sched().lock().unwrap();
        s.epoch = s.epoch.wrapping_add(1);
    }
    cv().notify_all();
}

pub fn get_simulation_speed_percent() -> u32 {
    let clock_lock = clock().lock().unwrap();
    let pct = ((clock_lock.scale_q32 as u128) * 100u128 / (ONE_Q32 as u128)) as u64;
    log::debug!("Get current simulation speed percent: {}", pct);
    pct as u32
}
