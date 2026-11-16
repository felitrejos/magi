use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::projects::clean;
use crate::bounded;

const ANSWER_WITHIN: Duration = Duration::from_secs(90);
const MAX_TITLES: usize = 3;
const MAX_TITLE_CHARS: usize = 80;
const SYSTEM: &str = "You name projects in the sidebar of a tool that lists coding sessions. \
For each new repo, choose the existing project it clearly belongs to, using that project's name \
exactly, or give it a short project name of its own, one to three words, written the way a \
person would. Reply with only a JSON object mapping each new repo to its project name.";

#[derive(Serialize)]
pub struct Request {
    pub projects: Vec<Project>,
    pub new: Vec<NewRepo>,
}

#[derive(Serialize)]
pub struct Project {
    pub name: String,
    pub repos: Vec<String>,
}

#[derive(Serialize)]
pub struct NewRepo {
    pub repo: String,
    pub sessions: Vec<String>,
}

#[derive(Deserialize)]
struct Output {
    result: String,
    #[serde(default)]
    is_error: bool,
}

pub fn new_repo(repo: &str, titles: impl Iterator<Item = String>) -> NewRepo {
    NewRepo {
        repo: repo.to_string(),
        sessions: titles
            .take(MAX_TITLES)
            .map(|title| title.chars().take(MAX_TITLE_CHARS).collect())
            .collect(),
    }
}

pub fn ask(
    claude: &Path,
    workdir: &Path,
    request: &Request,
) -> Result<BTreeMap<String, String>, String> {
    let input = serde_json::to_string(request).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(workdir).map_err(|error| error.to_string())?;
    let output = bounded::output(
        Command::new(claude)
            .current_dir(workdir)
            .args([
                "-p",
                "--model",
                "haiku",
                "--tools",
                "",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--no-session-persistence",
                "--output-format",
                "json",
                "--system-prompt",
                SYSTEM,
            ])
            .arg(input)
            .stdin(Stdio::null()),
        ANSWER_WITHIN,
    )
    .map_err(|error| format!("can't run claude: {error}"))?;
    if !output.status.success() {
        return Err("claude couldn't name them".into());
    }
    let output: Output = serde_json::from_slice(&output.stdout)
        .map_err(|_| "claude answered in a form magi can't read".to_string())?;
    if output.is_error {
        return Err("claude couldn't name them".into());
    }
    let asked: Vec<&str> = request.new.iter().map(|repo| repo.repo.as_str()).collect();
    Ok(names(&output.result, &asked))
}

fn names(result: &str, asked: &[&str]) -> BTreeMap<String, String> {
    let (Some(start), Some(end)) = (result.find('{'), result.rfind('}')) else {
        return BTreeMap::new();
    };
    let Ok(answer) = serde_json::from_str::<BTreeMap<String, String>>(&result[start..=end]) else {
        return BTreeMap::new();
    };
    answer
        .into_iter()
        .filter_map(|(repo, name)| {
            let repo = asked
                .iter()
                .find(|asked| asked.eq_ignore_ascii_case(&repo))?
                .to_string();
            let name = clean(&name);
            (!name.is_empty()).then_some((repo, name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_for_the_repos_asked_about_are_kept_and_cleaned() {
        let result = "```json\n{\"Ping\": \"Network Pong\", \"other\": \"Nope\", \"notes\": \" \\u0007 \"}\n```";
        assert_eq!(
            names(result, &["ping", "notes"]),
            BTreeMap::from([("ping".to_string(), "Network Pong".to_string())])
        );
        assert!(names("I can't help with that", &["ping"]).is_empty());
    }

    #[test]
    fn a_new_repo_carries_at_most_three_short_session_titles() {
        let long = "x".repeat(200);
        let repo = new_repo(
            "ping",
            ["a", long.as_str(), "c", "d"].into_iter().map(String::from),
        );
        assert_eq!(repo.sessions.len(), 3);
        assert_eq!(repo.sessions[1].chars().count(), MAX_TITLE_CHARS);
    }
}
