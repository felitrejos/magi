use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::proto::{Agent, Plan, Window};

const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
const ASK_AFTER_RESET: Duration = Duration::from_secs(30);

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
        let since = self.asked_at.map(|at| now.duration_since(at));
        let reset = has_reset(
            self.answer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref(),
            now_secs,
        );
        let due = since.is_none_or(|since| since >= ASK_EVERY || reset && since >= ASK_AFTER_RESET);
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

fn freshest(seen: Option<Reading>, asked: Option<Reading>) -> Option<Reading> {
    seen.into_iter().chain(asked).max_by_key(|(at, _)| *at)
}

fn current(windows: Vec<Window>, now_secs: u64) -> Vec<Window> {
    let mut windows: Vec<Window> = windows
        .into_iter()
        .map(|window| roll_forward(window, now_secs))
        .collect();
    windows.sort_by_key(|window| window.minutes);
    windows
}

fn roll_forward(window: Window, now_secs: u64) -> Window {
    if window.resets_at > now_secs {
        return window;
    }
    let period = (u64::from(window.minutes) * 60).max(1);
    let missed = (now_secs - window.resets_at) / period + 1;
    Window {
        used: 0,
        resets_at: window.resets_at + missed * period,
        ..window
    }
}

fn has_reset(reading: Option<&Reading>, now_secs: u64) -> bool {
    reading.is_some_and(|(_, windows)| windows.iter().any(|window| window.resets_at <= now_secs))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_past_its_reset_reads_empty_until_the_next_one_and_old_readings_lose() {
        let window = |used, resets_at| Window {
            minutes: 300,
            used,
            resets_at,
        };
        assert_eq!(
            current(vec![window(5_200, 2_000)], 1_000),
            vec![window(5_200, 2_000)]
        );
        assert_eq!(
            current(vec![window(5_200, 2_000)], 2_000),
            vec![window(0, 20_000)]
        );
        assert_eq!(
            current(vec![window(5_200, 2_000)], 40_000),
            vec![window(0, 56_000)]
        );
        let fresh = plan(
            Agent::ClaudeCode,
            Some((10, vec![window(100, 99_999)])),
            None,
            Instant::now(),
            50,
        );
        assert_eq!(
            fresh.map(|plan| (plan.as_of, plan.windows[0].used)),
            Some((10, 100))
        );
        assert!(has_reset(Some(&(0, vec![window(1, 50)])), 50));
        assert!(!has_reset(Some(&(0, vec![window(1, 51)])), 50));
    }
}
