use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::bounded;
use crate::proto::{AgentSession, Checks, PrState, PullRequest, Review};

const ASK_EVERY: Duration = Duration::from_secs(120);
const ASK_WITHIN: Duration = Duration::from_secs(15);
const MAX_ASKING: usize = 4;
const FIELDS: &str = "number,state,isDraft,reviewDecision,statusCheckRollup,url";

type Key = (String, String);

struct Known {
    asked: Instant,
    pr: Option<PullRequest>,
    asking: bool,
}

#[derive(Clone, Default)]
pub struct Prs {
    known: Arc<Mutex<HashMap<Key, Known>>>,
}

impl Prs {
    pub fn fill(&self, sessions: &mut [AgentSession], cwds: &HashMap<String, String>) {
        let Some(gh) = crate::handoff::find("gh", &crate::handoff::search_dirs()) else {
            return;
        };
        let due = self.update(sessions, cwds, Instant::now());
        for key in due {
            self.ask(gh.clone(), key);
        }
    }

    fn update(
        &self,
        sessions: &mut [AgentSession],
        cwds: &HashMap<String, String>,
        now: Instant,
    ) -> Vec<Key> {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let mut asking = known.values().filter(|known| known.asking).count();
        let mut due = Vec::new();
        for session in sessions.iter_mut() {
            let (Some(branch), Some(cwd)) = (&session.branch, cwds.get(&session.id)) else {
                continue;
            };
            let key = (cwd.clone(), branch.clone());
            let stale = known
                .get(&key)
                .is_none_or(|entry| !entry.asking && now.duration_since(entry.asked) >= ASK_EVERY);
            if stale && asking < MAX_ASKING {
                asking += 1;
                let entry = known.entry(key.clone()).or_insert(Known {
                    asked: now,
                    pr: None,
                    asking: true,
                });
                entry.asking = true;
                entry.asked = now;
                due.push(key.clone());
            }
            session.pr = known.get(&key).and_then(|entry| entry.pr.clone());
        }
        due
    }

    fn ask(&self, gh: PathBuf, key: Key) {
        let known = Arc::clone(&self.known);
        let asked = key.clone();
        let spawned = thread::Builder::new().name("gh".into()).spawn(move || {
            let (cwd, branch) = &asked;
            let pr = bounded::output(
                Command::new(gh)
                    .args(["pr", "view", branch, "--json", FIELDS])
                    .current_dir(cwd),
                ASK_WITHIN,
            )
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| parse(&output.stdout));
            answered(&known, &asked, pr);
        });
        if spawned.is_err() {
            answered(&self.known, &key, None);
        }
    }
}

fn answered(known: &Mutex<HashMap<Key, Known>>, key: &Key, pr: Option<PullRequest>) {
    if let Some(entry) = known
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mut(key)
    {
        entry.pr = pr;
        entry.asking = false;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct View {
    number: u32,
    state: String,
    is_draft: bool,
    #[serde(default)]
    review_decision: String,
    #[serde(default)]
    status_check_rollup: Vec<Check>,
    url: String,
}

#[derive(Deserialize)]
struct Check {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

fn parse(json: &[u8]) -> Option<PullRequest> {
    let view: View = serde_json::from_slice(json).ok()?;
    let state = match (view.state.as_str(), view.is_draft) {
        ("MERGED", _) => PrState::Merged,
        ("CLOSED", _) => PrState::Closed,
        (_, true) => PrState::Draft,
        _ => PrState::Open,
    };
    let review = match view.review_decision.as_str() {
        "APPROVED" => Some(Review::Approved),
        "CHANGES_REQUESTED" => Some(Review::ChangesRequested),
        "REVIEW_REQUIRED" => Some(Review::Required),
        _ => None,
    };
    Some(PullRequest {
        number: view.number,
        state,
        checks: checks(&view.status_check_rollup),
        review,
        url: view.url,
    })
}

fn checks(rollup: &[Check]) -> Checks {
    const FAILED: [&str; 5] = [
        "FAILURE",
        "ERROR",
        "CANCELLED",
        "TIMED_OUT",
        "ACTION_REQUIRED",
    ];
    if rollup.is_empty() {
        return Checks::None;
    }
    let failed = rollup.iter().any(|check| {
        check
            .conclusion
            .as_deref()
            .is_some_and(|c| FAILED.contains(&c))
            || check.state.as_deref().is_some_and(|s| FAILED.contains(&s))
    });
    let running = rollup.iter().any(|check| {
        check
            .status
            .as_deref()
            .is_some_and(|status| status != "COMPLETED")
            || matches!(check.state.as_deref(), Some("PENDING" | "EXPECTED"))
    });
    if failed {
        Checks::Failing
    } else if running {
        Checks::Running
    } else {
        Checks::Passing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_is_asked_about_once_until_its_answer_comes_back() {
        let prs = Prs::default();
        let now = Instant::now();
        let mut sessions = vec![AgentSession {
            branch: Some("fix".into()),
            ..AgentSession::example("s1")
        }];
        let cwds = HashMap::from([("s1".to_string(), "/repo".to_string())]);
        assert_eq!(prs.update(&mut sessions, &cwds, now).len(), 1);
        assert!(
            prs.update(&mut sessions, &cwds, now + ASK_EVERY * 2)
                .is_empty()
        );
        let key = ("/repo".to_string(), "fix".to_string());
        let pr = parse(br#"{"isDraft":false,"number":3,"reviewDecision":"","state":"OPEN","statusCheckRollup":[],"url":"u"}"#);
        answered(&prs.known, &key, pr);
        assert!(prs.update(&mut sessions, &cwds, now).is_empty());
        assert_eq!(sessions[0].pr.as_ref().map(|pr| pr.number), Some(3));
        assert_eq!(prs.update(&mut sessions, &cwds, now + ASK_EVERY).len(), 1);
    }

    #[test]
    fn a_pull_request_reads_its_state_review_and_checks() {
        let pr = parse(br#"{"isDraft":true,"number":42,"reviewDecision":"REVIEW_REQUIRED","state":"OPEN","statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}],"url":"https://github.com/o/r/pull/42"}"#).unwrap();
        assert_eq!(
            (pr.number, pr.state, pr.checks, pr.review),
            (42, PrState::Draft, Checks::Running, Some(Review::Required))
        );
        let merged = parse(br#"{"isDraft":false,"number":1,"reviewDecision":"","state":"MERGED","statusCheckRollup":[],"url":"u"}"#).unwrap();
        assert_eq!(
            (merged.state, merged.checks, merged.review),
            (PrState::Merged, Checks::None, None)
        );
        let failing = parse(br#"{"isDraft":false,"number":7,"reviewDecision":"APPROVED","state":"OPEN","statusCheckRollup":[{"__typename":"StatusContext","state":"FAILURE"},{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}],"url":"u"}"#).unwrap();
        assert_eq!(
            (failing.checks, failing.review),
            (Checks::Failing, Some(Review::Approved))
        );
        assert_eq!(parse(b"no pull requests found"), None);
    }
}
