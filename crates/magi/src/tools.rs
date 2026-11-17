use std::process::Command;
use std::time::Duration;

use crate::{bounded, handoff};

const BUILD_TOOLS: [&str; 10] = [
    "cargo",
    "go",
    "node",
    "python3",
    "swift",
    "xcodebuild",
    "gcc",
    "clang",
    "make",
    "nvidia-smi",
];
const CONTAINERS: [&str; 2] = ["docker", "podman"];
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

pub fn available() -> Vec<String> {
    let dirs = handoff::search_dirs();
    let containers = CONTAINERS.into_iter().filter(|name| {
        handoff::find(name, &dirs).is_some_and(|program| {
            bounded::output(Command::new(program).arg("info"), ANSWER_WITHIN)
                .is_ok_and(|output| output.status.success())
        })
    });
    BUILD_TOOLS
        .into_iter()
        .filter(|name| handoff::find(name, &dirs).is_some())
        .chain(containers)
        .map(str::to_string)
        .collect()
}
