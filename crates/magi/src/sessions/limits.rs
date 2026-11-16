use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::proto::{Agent, BasisPoints, FULL, Plan, Window};

const ASK_EVERY: Duration = Duration::from_secs(5 * 60);

pub type Reading = (u64, Vec<Window>);
type Ask = Arc<dyn Fn(u64) -> Option<Vec<Window>> + Send + Sync>;

pub struct Limits {
    ask: Ask,
    asked_at: Option<Instant>,
    asking: Arc<AtomicBool>,
    answer: Arc<Mutex<Option<Reading>>>,
}

impl Limits {
    pub fn new(ask: impl Fn(u64) -> Option<Vec<Window>> + Send + Sync + 'static) -> Self {
        Self {
            ask: Arc::new(ask),
            asked_at: None,
            asking: Arc::new(AtomicBool::new(false)),
            answer: Arc::new(Mutex::new(None)),
        }
    }

    pub fn latest(&mut self, now: Instant, now_secs: u64) -> Option<Reading> {
        let due = self
            .asked_at
            .is_none_or(|at| now.duration_since(at) >= ASK_EVERY);
        if due && !self.asking.swap(true, Ordering::AcqRel) {
            self.asked_at = Some(now);
            let (ask, asking, answer) =
                (self.ask.clone(), self.asking.clone(), self.answer.clone());
            thread::spawn(move || {
                if let Some(windows) = ask(now_secs) {
                    *answer.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((now_secs, windows));
                }
                asking.store(false, Ordering::Release);
            });
        }
        self.answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

pub fn percent(value: f64) -> BasisPoints {
    (value.clamp(0.0, 100.0) * f64::from(FULL) / 100.0).round() as BasisPoints
}

fn freshest(seen: Option<Reading>, asked: Option<Reading>) -> Option<Reading> {
    seen.into_iter().chain(asked).max_by_key(|(at, _)| *at)
}

fn current(windows: Vec<Window>, now_secs: u64) -> Vec<Window> {
    let mut windows: Vec<Window> = windows
        .into_iter()
        .filter(|window| window.resets_at > now_secs)
        .collect();
    windows.sort_by_key(|window| window.minutes);
    windows
}

pub const FRESH_SECS: u64 = 5 * 60;

pub fn plan(
    agent: Agent,
    seen: Option<Reading>,
    limits: Option<&mut Limits>,
    now: Instant,
    now_secs: u64,
) -> Option<Plan> {
    let fresh = seen
        .as_ref()
        .is_some_and(|(at, _)| now_secs.saturating_sub(*at) < FRESH_SECS);
    let asked = limits
        .filter(|_| !fresh)
        .and_then(|limits| limits.latest(now, now_secs));
    let (as_of, windows) = freshest(seen, asked)?;
    let windows = current(windows, now_secs);
    (!windows.is_empty()).then_some(Plan {
        agent,
        windows,
        as_of,
    })
}
