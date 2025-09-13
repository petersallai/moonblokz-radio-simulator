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
    last_set_percent: u32,     // exact percent requested by caller (avoids FP truncation off-by-one)
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
            last_set_percent: 100,
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
    // If rebasing moved origin_virtual_ticks past v_target, treat it as due now instead of wrapping
    // (wrapping would create an enormous virt_dt and thus absurd wait durations).
    let virt_dt = match v_target.checked_sub(clock_lock.origin_virtual_ticks) {
        Some(dt) => dt,
        None => return clock_lock.origin_real, // already due
    };
    let real_ticks = ((virt_dt as u128) * (ONE_Q32 as u128) / (clock_lock.scale_q32 as u128)) as u64;
    let real_ns = (real_ticks as u128) * 1_000_000_000u128 / (tick_hz() as u128);
    // Clamp to avoid potential u128 -> u64 truncation on very long durations
    let real_ns_u64 = real_ns.min(u64::MAX as u128) as u64;
    clock_lock.origin_real + Duration::from_nanos(real_ns_u64)
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
                let guard = cv().wait(guard).unwrap();
                drop(guard);
                continue;
            }
            let (&next_at, _) = guard.queue.iter().next().unwrap();
            let snapshot_epoch = guard.epoch;
            drop(guard);
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
            let (new_guard, _timeout_res) = cv().wait_timeout(guard, wait_dur).unwrap();
            guard = new_guard;
            // If epoch changed (speed slider moved) or we were notified, simply iterate again.
            if guard.epoch != snapshot_epoch {
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
        v
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        ensure_scheduler_thread();
        let mut guard = sched().lock().unwrap();
        guard.queue.entry(at).or_default().push(waker.clone());
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
        return;
    }
    let r_now = real_now();
    // Virtual 'now' under the OLD mapping (before mutation)
    let v_now_old = map_real_to_virtual(r_now);
    let new_scale_q32 = ((percent as u128) * (ONE_Q32 as u128) / 100u128) as u64;

    // Adjust ONLY origin_real to preserve continuity, keeping origin_virtual_ticks unchanged so
    // existing queued deadlines never become "in the past" via an origin shift (which previously
    // caused wrapping_sub underflow and gigantic wait durations).
    {
        let mut c = clock().lock().unwrap();
        let origin_virtual = c.origin_virtual_ticks; // unchanged
        let delta_v = v_now_old.saturating_sub(origin_virtual) as u128; // ticks
        // real_elapsed_new_ticks = delta_v / new_scale  (since v = origin + real*scale)
        let real_elapsed_new_ticks = if new_scale_q32 == 0 {
            0
        } else {
            delta_v * (ONE_Q32 as u128) / (new_scale_q32 as u128)
        };
        let real_elapsed_new_ns = real_elapsed_new_ticks * 1_000_000_000u128 / (tick_hz() as u128);
        let dur = Duration::from_nanos(real_elapsed_new_ns.min(u64::MAX as u128) as u64);
        // Set new origin_real = r_now - dur (checked to avoid panic if dur > r_now elapsed span)
        if let Some(new_origin_real) = r_now.checked_sub(dur) {
            c.origin_real = new_origin_real;
        } else {
            // Fallback: if subtraction underflows (extremely large dur), just anchor at now.
            c.origin_real = r_now;
        }
        c.scale_q32 = new_scale_q32;
        c.last_set_percent = percent; // record exact requested percent
    }
    // Epoch bump so scheduler re-evaluates earliest deadline with new mapping immediately.
    {
        let mut s = sched().lock().unwrap();
        s.epoch = s.epoch.wrapping_add(1);
    }
    cv().notify_all();
}

pub fn get_simulation_speed_percent() -> u32 {
    let clock_lock = clock().lock().unwrap();
    let pct = clock_lock.last_set_percent;
    pct
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serialize tests touching global clock/scheduler state
    static TEST_GUARD: StdMutex<()> = StdMutex::new(());

    #[test]
    fn continuity_on_speed_change_preserves_mapping() {
        let _g = TEST_GUARD.lock().unwrap();
        // Reset to a known speed
        set_simulation_speed_percent(100);
        let anchor = real_now();
        let v_before = map_real_to_virtual(anchor);
        // Change speed and ensure mapping of the SAME real instant is (nearly) identical
        set_simulation_speed_percent(400);
        let v_after = map_real_to_virtual(anchor);
        // Allow tiny tolerance in ticks due to integer rounding
        let diff = if v_after > v_before { v_after - v_before } else { v_before - v_after };
        assert!(diff <= tick_hz() / 100, "virtual mapping changed too much on speed change: diff={} ticks", diff);
    }

    #[test]
    fn virtual_to_real_scales_inverse_with_speed() {
    let _g = TEST_GUARD.lock().unwrap();
    // Reset to a known speed then set desired
    set_simulation_speed_percent(100);
    set_simulation_speed_percent(200); // x2 virtual vs real
        let now_r = real_now();
        let now_v = map_real_to_virtual(now_r);
        // Target +0.2 virtual seconds
        let dt_v_ticks = (tick_hz() as f64 * 0.2) as u64;
        let target_v = now_v.wrapping_add(dt_v_ticks);
        let target_r = map_virtual_to_real(target_v);
        let real_dt = target_r.duration_since(now_r);
        let expected_secs = 0.2 / 2.0; // half a real second factor since 200%
        let diff = (real_dt.as_secs_f64() - expected_secs).abs();
        assert!(diff < 0.01, "expected ~{expected_secs}s, got {:?}", real_dt);
    }

    #[test]
    fn map_virtual_to_real_handles_past_targets() {
        let _g = TEST_GUARD.lock().unwrap();
        set_simulation_speed_percent(100);
        let c = clock().lock().unwrap();
        let origin_v = c.origin_virtual_ticks;
        let origin_r = c.origin_real;
        drop(c);
    // v_target just before origin_virtual should be treated as due now (origin_real).
    // Use saturating_sub to avoid wrap-around when origin_v == 0.
    let v_target = origin_v.saturating_sub(1);
        let r = map_virtual_to_real(v_target);
        // Within small ns tolerance
        let d = if r > origin_r { r - origin_r } else { origin_r - r };
        assert!(d < Duration::from_millis(1));
    }
}
