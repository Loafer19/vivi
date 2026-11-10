//! One shared scheduler for the UI thread's repeating work (plugin ticks, tray and
//! MPRIS updates, …). Player events and the control socket wake the UI when something
//! arrives rather than polling here. Callers start a task only while it has work and
//! [`stop`] (or [`clear`]) it when idle so the shared timer can go to sleep.
//!
//! Every task keeps its own interval, but they all run off a single [`slint::Timer`]
//! that is armed for the next due task. A task that falls due within a quarter of its
//! interval (at most [`MAX_SLACK`]) of an earlier wake-up runs with it, so tasks with
//! nearby intervals share wake-ups instead of each waking the event loop. Frame tasks
//! ([`frames`]) never run early. [`stop`] ends a task; the timer stops when none is left.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How much earlier than due a task may run to share a wake-up.
const MAX_SLACK: Duration = Duration::from_millis(25);

/// A repeating task; [`stop`] ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Task(u64);

type Job = Rc<RefCell<dyn FnMut()>>;

struct Entry {
    id: u64,
    every: Duration,
    slack: Duration,
    due: Instant,
    job: Job,
    alive: Rc<Cell<bool>>,
}

#[derive(Default)]
struct Sched {
    next_id: u64,
    tasks: Vec<Entry>,
    /// When the timer is armed for (None: stopped).
    armed: Option<Instant>,
}

thread_local! {
    static SCHED: RefCell<Sched> = RefCell::new(Sched::default());
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Run `f` every `every` from now on (the first run is one interval away). It may run
/// up to a quarter interval early (at most 25 ms) to share a wake-up with another task.
pub fn every(every: Duration, f: impl FnMut() + 'static) -> Task {
    add(every, (every / 4).min(MAX_SLACK), f)
}

/// Like [`every`], but never early (animation frames).
pub fn frames(every: Duration, f: impl FnMut() + 'static) -> Task {
    add(every, Duration::ZERO, f)
}

/// End `task`; it does not run again (also when stopped from inside its own run).
pub fn stop(task: Task) {
    SCHED.with(|s| {
        let mut s = s.borrow_mut();
        s.tasks.retain(|e| {
            let keep = e.id != task.0;
            if !keep {
                e.alive.set(false);
            }
            keep
        });
    });
    rearm();
}

/// Start `f` every `every` unless `slot` already holds a running task.
pub fn ensure(slot: &Cell<Option<Task>>, every: Duration, f: impl FnMut() + 'static) {
    if slot.get().is_some() {
        return;
    }
    slot.set(Some(self::every(every, f)));
}

/// Stop and forget the task in `slot`, if any.
pub fn clear(slot: &Cell<Option<Task>>) {
    if let Some(t) = slot.take() {
        stop(t);
    }
}

fn add(every: Duration, slack: Duration, f: impl FnMut() + 'static) -> Task {
    let every = every.max(Duration::from_millis(1));
    let id = SCHED.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.tasks.push(Entry {
            id,
            every,
            slack,
            due: Instant::now() + every,
            job: Rc::new(RefCell::new(f)),
            alive: Rc::new(Cell::new(true)),
        });
        id
    });
    rearm();
    Task(id)
}

/// The tasks due at `now` (with their slack), rescheduled one interval later.
fn take_due(s: &mut Sched, now: Instant) -> Vec<(Job, Rc<Cell<bool>>)> {
    let mut due = Vec::new();
    for e in &mut s.tasks {
        if e.due <= now + e.slack {
            due.push((e.job.clone(), e.alive.clone()));
            // From this run, so a task that ran early with another one keeps sharing its
            // wake-ups, and a stall does not replay the missed runs in a burst.
            e.due = now + e.every;
        }
    }
    due
}

fn next_due(s: &Sched) -> Option<Instant> {
    s.tasks.iter().map(|e| e.due).min()
}

fn fire() {
    let now = Instant::now();
    let due = SCHED.with(|s| {
        let mut s = s.borrow_mut();
        s.armed = None;
        take_due(&mut s, now)
    });
    // Run outside the borrow: a task may add or stop tasks.
    for (job, alive) in due {
        if alive.get() {
            if let Ok(mut f) = job.try_borrow_mut() {
                f();
            }
        }
    }
    rearm();
}

/// Arm the timer for the earliest due task (or stop it when there is none).
fn rearm() {
    let next = SCHED.with(|s| {
        let mut s = s.borrow_mut();
        let next = next_due(&s);
        if next == s.armed {
            return None;
        }
        s.armed = next;
        Some(next)
    });
    let Some(next) = next else { return };
    TIMER.with(|t| match next {
        Some(at) => t.start(
            slint::TimerMode::SingleShot,
            at.saturating_duration_since(Instant::now()),
            fire,
        ),
        None => t.stop(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(every_ms: &[(u64, u64)], t0: Instant) -> Sched {
        let mut s = Sched::default();
        for (i, &(every, slack)) in every_ms.iter().enumerate() {
            let every = Duration::from_millis(every);
            s.tasks.push(Entry {
                id: i as u64,
                every,
                slack: Duration::from_millis(slack),
                due: t0 + every,
                job: Rc::new(RefCell::new(|| {})),
                alive: Rc::new(Cell::new(true)),
            });
        }
        s
    }

    /// Wake-ups in `ms` of simulated time, waking exactly when the scheduler asks.
    fn wakeups(s: &mut Sched, t0: Instant, ms: u64) -> (usize, Vec<usize>) {
        let mut runs = vec![0; s.tasks.len()];
        let mut n = 0;
        while let Some(at) = next_due(s).filter(|&at| at <= t0 + Duration::from_millis(ms)) {
            n += 1;
            let before: Vec<Instant> = s.tasks.iter().map(|e| e.due).collect();
            take_due(s, at);
            for (i, e) in s.tasks.iter().enumerate() {
                if e.due != before[i] {
                    runs[i] += 1;
                }
            }
        }
        (n, runs)
    }

    #[test]
    fn nearby_intervals_share_wakeups() {
        let t0 = Instant::now();
        // The event poll (30 ms) and the control socket (40 ms, slack 10 ms).
        let mut s = sched(&[(30, 7), (40, 10)], t0);
        let (n, runs) = wakeups(&mut s, t0, 1200);
        assert_eq!(runs[0], 40);
        // The 40 ms task runs with the 30 ms one: no wake-ups of its own (two separate
        // timers woke the loop 70 times).
        assert_eq!(n, 40);
        assert_eq!(runs[1], 40);
    }

    #[test]
    fn frames_are_never_early() {
        let t0 = Instant::now();
        let mut s = sched(&[(33, 0)], t0);
        let first = next_due(&s).unwrap();
        assert!(take_due(&mut s, first - Duration::from_millis(1)).is_empty());
        assert_eq!(take_due(&mut s, first).len(), 1);
    }

    #[test]
    fn a_stall_does_not_replay_missed_runs() {
        let t0 = Instant::now();
        let mut s = sched(&[(100, 0)], t0);
        let late = t0 + Duration::from_secs(2);
        assert_eq!(take_due(&mut s, late).len(), 1);
        assert!(take_due(&mut s, late).is_empty());
        assert_eq!(next_due(&s), Some(late + Duration::from_millis(100)));
    }
}
