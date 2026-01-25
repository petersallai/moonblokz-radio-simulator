//! Scaled virtual time driver for Embassy.
//!
//! This module implements a global `embassy_time_driver::Driver` that maps
//! real (host) time to a virtual clock whose speed can be adjusted at runtime
//! via a percentage slider. The mapping preserves virtual-time continuity when
//! the speed changes and avoids timer "bursts" or starvation by:
//!
//! - Rebasing only the real origin on speed changes, keeping the virtual origin
//!   fixed so scheduled deadlines never wrap into the past.
//! - Slicing scheduler waits (<= 25 ms) and bumping an epoch flag on speed
//!   updates so the scheduler re-evaluates promptly.
//! - Guarding conversions between real/virtual to avoid under/overflow.
//!
//! Public helpers `set_simulation_speed_percent` and
//! `get_simulation_speed_percent` provide an exact set/get contract (no FP
//! drift). The driver is registered with `time_driver_impl!` and is used by
//! embassy-time throughout the app.
//!
//! ## Lock Ordering Rules (CRITICAL for deadlock prevention)
//!
//! To prevent lock inversion deadlocks, all code MUST follow this strict ordering:
//!
//! 1. **CLOCK** must always be acquired BEFORE **SCHED** (never the reverse)
//! 2. Never hold both locks simultaneously if possible
//! 3. If both are needed, acquire CLOCK first, extract needed data, drop it, then acquire SCHED
//!
//! Example of CORRECT ordering:
//! ```rust
//! let data = { let c = clock().lock().unwrap(); extract_data(&c) }; // CLOCK acquired & dropped
//! let mut s = sched().lock().unwrap(); // Now safe to acquire SCHED
//! use_data(&mut s, data);
//! ```
//!
//! Example of INCORRECT ordering (DEADLOCK RISK):
//! ```rust
//! let s = sched().lock().unwrap();  // SCHED acquired first
//! let c = clock().lock().unwrap();  // CLOCK acquired second - DEADLOCK!
//! ```

use core::task::Waker;
use embassy_time_driver::{Driver, TICK_HZ, time_driver_impl};
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant as StdInstant};

// Fixed-point Q32.32 for speed scaling. 1.0 == 1<<32
const ONE_Q32: u64 = 1u64 << 32;

#[derive(Debug)]
struct ScaledClock {
    /// Host reference time corresponding to `origin_virtual_ticks`.
    origin_real: StdInstant,
    /// Virtual time origin in Embassy ticks (monotonic, wraps on u64).
    origin_virtual_ticks: u64,
    /// Q32.32 scale: virtual_dt = real_dt * scale_q32.
    /// Example: 0.5x speed => 0.5 * ONE_Q32.
    scale_q32: u64,
    /// Last exact percent set by the UI; returned verbatim by `get_…` to
    /// avoid floating-point roundoff surprises.
    last_set_percent: u32,
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

/// Map a real (host) timestamp to virtual Embassy ticks using the current
/// scale and origins. Preserves continuity across speed changes by construction.
///
/// The mapping formula is:
/// ```text
/// virtual_ticks = origin_virtual + (real_elapsed_ticks * scale)
/// ```
///
/// This ensures that changing the speed scale only affects future time advancement,
/// not already-scheduled deadlines (origin_virtual remains fixed during speed changes).
///
/// ## Lock Safety
/// Acquires CLOCK lock only. Safe to call while holding SCHED lock per lock ordering rules.
fn map_real_to_virtual(r: StdInstant) -> u64 {
    // LOCK ORDERING: CLOCK only (safe to call from any context)
    let clock_lock = clock().lock().unwrap();
    let real_dt = r.saturating_duration_since(clock_lock.origin_real);
    let real_ticks = (real_dt.as_nanos() as u128 * tick_hz() as u128 / 1_000_000_000u128) as u64;
    let scaled = ((real_ticks as u128) * (clock_lock.scale_q32 as u128) / (ONE_Q32 as u128)) as u64;
    clock_lock.origin_virtual_ticks.wrapping_add(scaled)
}

/// Map a virtual Embassy tick target back to a real (host) timestamp.
///
/// Inverse operation of `map_real_to_virtual`. The formula is:
/// ```text
/// real_time = origin_real + (virtual_delta / scale)
/// ```
///
/// If the target is before the current virtual origin (due to a rebase), it is
/// treated as "due now" to avoid underflow and absurd waits.
///
/// This can happen when speed changes move origin_virtual past old deadlines,
/// but we intentionally keep origin_virtual fixed and only adjust origin_real
/// to preserve continuity and prevent negative virtual deltas.
///
/// ## Lock Safety
/// Acquires CLOCK lock only. Safe to call while holding SCHED lock per lock ordering rules.
fn map_virtual_to_real(v_target: u64) -> StdInstant {
    // LOCK ORDERING: CLOCK only (safe to call from any context)
    let clock_lock = clock().lock().unwrap();
    // If rebasing moved origin_virtual_ticks past v_target, treat it as due now instead of wrapping
    // (wrapping would create an enormous virt_dt and thus absurd wait durations).
    let virt_dt = match v_target.checked_sub(clock_lock.origin_virtual_ticks) {
        Some(dt) => dt,
        None => return clock_lock.origin_real, // already due
    };
    let real_ticks =
        ((virt_dt as u128) * (ONE_Q32 as u128) / (clock_lock.scale_q32 as u128)) as u64;
    let real_ns = (real_ticks as u128) * 1_000_000_000u128 / (tick_hz() as u128);
    // Clamp to avoid potential u128 -> u64 truncation on very long durations
    let real_ns_u64 = real_ns.min(u64::MAX as u128) as u64;
    clock_lock.origin_real + Duration::from_nanos(real_ns_u64)
}

/// Start the dedicated scheduler thread once. Safe to call repeatedly.
///
/// The scheduler thread is responsible for waking Embassy async tasks when their
/// virtual deadlines are reached. It runs in a loop, waiting for the next due time
/// and waking all tasks scheduled for that time.
fn ensure_scheduler_thread() {
    SCHEDULER_STARTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("embassy-time-scheduler".into())
            .spawn(move || scheduler_thread())
            .expect("failed to start embassy-time scheduler thread");
    });
}

/// Waits for the next due virtual deadline and wakes registered wakers.
///
/// Key properties:
/// - Strictly follows CLOCK-before-SCHED lock ordering to prevent deadlocks.
/// - Never holds both locks simultaneously - extracts data from one before acquiring the other.
/// - Waits are sliced (<= MAX_WAIT_SLICE) so speed changes reflect quickly
///   even if a notify is missed.
/// - Uses an `epoch` counter to detect speed changes between wait and wakeup.
///
/// ## Lock Ordering Safety
/// This function carefully respects the CLOCK → SCHED ordering:
/// 1. Acquires SCHED, extracts virtual deadline, releases SCHED
/// 2. Calls map_virtual_to_real (which acquires CLOCK independently)
/// 3. Re-acquires SCHED for timed wait if needed
/// This pattern prevents SCHED → CLOCK inversion which would deadlock.
fn scheduler_thread() {
    // Maximum slice to wait so scale changes apply promptly even if a notify is missed
    // 25ms chosen to balance UI responsiveness vs CPU overhead from frequent wake-ups
    const MAX_WAIT_SLICE: Duration = Duration::from_millis(25);
    loop {
        // STEP 1: Extract next deadline from SCHED without holding lock during CLOCK access
        // LOCK ORDERING: Acquire SCHED, extract data, drop SCHED before calling map_virtual_to_real
        let (next_at, snapshot_epoch) = loop {
            let guard = sched().lock().unwrap();
            if guard.queue.is_empty() {
                // Wait for new items to be scheduled
                let guard = cv().wait(guard).unwrap();
                drop(guard);
                continue;
            }
            let (&next_at, _) = guard.queue.iter().next().unwrap();
            let snapshot_epoch = guard.epoch;
            drop(guard); // CRITICAL: Release SCHED before calling map_virtual_to_real
            break (next_at, snapshot_epoch);
        };

        // STEP 2: Convert virtual to real time (acquires CLOCK internally, but SCHED is released)
        // LOCK ORDERING: SCHED was released above, so map_virtual_to_real can safely acquire CLOCK
        let real_target = map_virtual_to_real(next_at);
        let now_r = real_now();

        if real_target > now_r {
            let mut wait_dur = real_target - now_r;
            if wait_dur > MAX_WAIT_SLICE {
                wait_dur = MAX_WAIT_SLICE;
            }
            // STEP 3: Timed wait (safe because we're not calling any functions that acquire CLOCK)
            // LOCK ORDERING: Re-acquire SCHED for timed wait only - no CLOCK access during this hold
            let mut guard = sched().lock().unwrap();
            let (new_guard, _timeout_res) = cv().wait_timeout(guard, wait_dur).unwrap();
            guard = new_guard;
            // If epoch changed (speed slider moved) or we were notified, iterate again
            if guard.epoch != snapshot_epoch {
                drop(guard);
                continue;
            }
            drop(guard);
            continue;
        }

        // STEP 4: Drain all due wakers
        // LOCK ORDERING: Compute virtual "now" BEFORE acquiring SCHED (CLOCK → SCHED order)
        // CRITICAL: map_real_to_virtual() acquires and releases CLOCK internally.
        // We MUST ensure CLOCK is fully released before acquiring SCHED below.
        // Any modification to map_real_to_virtual that holds CLOCK past return
        // will cause a deadlock here.
        let now_v = map_real_to_virtual(real_now()); // Acquires & releases CLOCK
        let mut ready: Vec<Waker> = Vec::new();
        {
            // Now safe to acquire SCHED since CLOCK was fully released above
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
        } // SCHED lock dropped here

        // STEP 5: Wake all ready tasks (no locks held during wake to prevent blocking)
        for w in ready.into_iter() {
            w.wake();
        }
    }
}

struct ScaledDriver;

impl Driver for ScaledDriver {
    /// Returns the current virtual time in Embassy ticks.
    fn now(&self) -> u64 {
        let v = map_real_to_virtual(real_now());
        v
    }

    /// Schedule a wakeup for a given virtual-tick timestamp.
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
/// Set the simulation speed in percent (1..=1000). Preserves virtual-time
/// continuity and updates the scheduler epoch for prompt responsiveness.
///
/// ## Lock Ordering
/// Acquires CLOCK then SCHED (correct ordering). Never acquires both simultaneously.
pub fn set_simulation_speed_percent(percent: u32) {
    let percent = percent.clamp(1, 1000);
    // Fast path: no-op if unchanged
    let current_pct = get_simulation_speed_percent();
    if current_pct == percent {
        return;
    }
    let r_now = real_now();
    // Virtual 'now' under the OLD mapping (before mutation)
    let v_now_old = map_real_to_virtual(r_now); // Acquires CLOCK internally
    let new_scale_q32 = ((percent as u128) * (ONE_Q32 as u128) / 100u128) as u64;

    // Adjust ONLY origin_real to preserve continuity, keeping origin_virtual_ticks unchanged so
    // existing queued deadlines never become "in the past" via an origin shift (which previously
    // caused wrapping_sub underflow and gigantic wait durations).
    // LOCK ORDERING: CLOCK first (step 1 of 2)
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
    } // CLOCK lock dropped here

    // LOCK ORDERING: SCHED second (step 2 of 2) - CLOCK was already released above
    // Epoch bump so scheduler re-evaluates earliest deadline with new mapping immediately.
    {
        let mut s = sched().lock().unwrap();
        s.epoch = s.epoch.wrapping_add(1);
    } // SCHED lock dropped here

    cv().notify_all();
}

/// Get the simulation speed as last set by `set_simulation_speed_percent`.
/// This returns the exact UI-facing value without floating-point rounding.
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
        let diff = if v_after > v_before {
            v_after - v_before
        } else {
            v_before - v_after
        };
        assert!(
            diff <= tick_hz() / 100,
            "virtual mapping changed too much on speed change: diff={} ticks",
            diff
        );
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
        let d = if r > origin_r {
            r - origin_r
        } else {
            origin_r - r
        };
        assert!(d < Duration::from_millis(1));
    }
}
