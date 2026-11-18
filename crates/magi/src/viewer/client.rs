use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::model::{Endpoint, LinkState, LinkUpdate, Model, Presence};
use super::{EVENT_BACKLOG, Event, Links, alerts, handle, net, topology};
use crate::proto::{AgentSession, Reply, Request, View};

const SETTLE_WITHIN: Duration = Duration::from_secs(3);
const MIN_ID_PREFIX: usize = 4;
const MIN_TITLE_PART: usize = 4;

pub struct Client {
    pub model: Model,
    links: Links,
    events: Receiver<Event>,
    inbox: VecDeque<(Endpoint, Answer)>,
}

pub enum Answer {
    Reply(Reply),
    Read(View),
}

#[derive(Clone)]
pub struct Found {
    pub endpoint: Endpoint,
    pub session: AgentSession,
}

impl Client {
    pub fn connect(port: u16) -> io::Result<Self> {
        let (sender, events) = mpsc::sync_channel(EVENT_BACKLOG);
        let links = Links {
            handles: HashMap::new(),
            next_generation: 0,
            port,
            socket: net::local_socket()?,
            sender: sender.clone(),
            rex: false,
            launch: None,
        };
        let started = Instant::now();
        let mut client = Self {
            model: Model::new(port, started, alerts::Alerts::in_memory(started)),
            links,
            events,
            inbox: VecDeque::new(),
        };
        client
            .links
            .reconcile(&mut client.model, &[Endpoint::Local])?;
        topology::spawn(sender)?;
        let deadline = started + SETTLE_WITHIN;
        let mut found_peers = false;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok(event) = client.events.recv_timeout(left) else {
                break;
            };
            found_peers |= matches!(event, Event::Topology(_));
            client.take(event)?;
            if found_peers && settled(&client.model, Instant::now()) {
                break;
            }
        }
        Ok(client)
    }

    pub fn session(&self, query: &str) -> Result<Found, String> {
        let agents = self.model.agents(Instant::now());
        let candidates: Vec<(&str, &str)> = agents
            .iter()
            .map(|agent| (agent.session.id.as_str(), agent.session.title.as_str()))
            .collect();
        let index = pick(&candidates, query)?;
        Ok(Found {
            endpoint: agents[index].endpoint,
            session: agents[index].session.clone(),
        })
    }

    pub fn machine(&self, name: &str) -> Result<String, String> {
        let rows = self.model.rows(Instant::now());
        rows.iter()
            .find(|row| row.name.eq_ignore_ascii_case(name))
            .map(|row| row.name.clone())
            .ok_or_else(|| {
                let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
                format!("no machine called {name:?}: there's {}", names.join(", "))
            })
    }

    pub fn endpoint(&self, machine: &str) -> Option<Endpoint> {
        self.model
            .rows(Instant::now())
            .iter()
            .find(|row| row.name == machine)
            .map(|row| row.endpoint)
    }

    pub fn ask(
        &mut self,
        endpoint: Endpoint,
        request: &Request,
        within: Duration,
    ) -> Result<String, String> {
        self.send(endpoint, request)?;
        self.reply(endpoint, within)
    }

    pub fn send(&self, endpoint: Endpoint, request: &Request) -> Result<(), String> {
        self.links
            .request(endpoint, request)
            .map_err(|error| format!("couldn't reach that machine: {error}"))
    }

    pub fn reply(&mut self, endpoint: Endpoint, within: Duration) -> Result<String, String> {
        let reply = self.answer(endpoint, within, |answer| match answer {
            Answer::Reply(reply) => Some(reply),
            Answer::Read(_) => None,
        })?;
        if reply.ok {
            Ok(reply.text)
        } else {
            Err(reply.text)
        }
    }

    pub fn answer<T>(
        &mut self,
        endpoint: Endpoint,
        within: Duration,
        wanted: impl Fn(Answer) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + within;
        loop {
            while let Some(index) = self.inbox.iter().position(|(from, _)| *from == endpoint) {
                let (_, answer) = self.inbox.remove(index).expect("index came from the queue");
                if let Some(found) = wanted(answer) {
                    return Ok(found);
                }
            }
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or("the machine didn't answer in time")?;
            self.wait(left).map_err(|error| error.to_string())?;
        }
    }

    pub fn wait(&mut self, within: Duration) -> io::Result<()> {
        if let Ok(event) = self.events.recv_timeout(within) {
            self.take(event)?;
            while let Ok(event) = self.events.try_recv() {
                self.take(event)?;
            }
        }
        Ok(())
    }

    fn take(&mut self, event: Event) -> io::Result<()> {
        let answer = match event {
            Event::Link(endpoint, _, LinkUpdate::Reply(reply)) => (endpoint, Answer::Reply(reply)),
            Event::Link(endpoint, _, LinkUpdate::Read(read)) => (endpoint, Answer::Read(read)),
            event => return handle(event, &mut self.model, &mut self.links).map(drop),
        };
        self.inbox.push_back(answer);
        Ok(())
    }
}

pub fn settled(model: &Model, now: Instant) -> bool {
    model.rows(now).iter().all(|row| match row.presence {
        Presence::Agent(link) => match link.state {
            LinkState::Live => row
                .machine()
                .is_some_and(|machine| machine.latest.is_some()),
            LinkState::Connecting => false,
            LinkState::Lost { .. } | LinkState::Rejected(_) => true,
        },
        Presence::Searching => false,
        Presence::NoAgent => true,
    })
}

pub fn pick(candidates: &[(&str, &str)], query: &str) -> Result<usize, String> {
    let wanted = query.to_lowercase();
    let matching = |found: &dyn Fn(&str, &str) -> bool| -> Vec<usize> {
        (0..candidates.len())
            .filter(|index| found(candidates[*index].0, &candidates[*index].1.to_lowercase()))
            .collect()
    };
    let mut found = matching(&|id, _| query.len() >= MIN_ID_PREFIX && id.starts_with(query));
    if found.is_empty() {
        found = matching(&|_, title| title == wanted);
    }
    if found.is_empty() {
        found = matching(&|_, title| {
            wanted.chars().count() >= MIN_TITLE_PART && title.contains(&wanted)
        });
    }
    match found.as_slice() {
        [index] => Ok(*index),
        [] => Err(format!(
            "no session matches {query:?}: see magi-overview sessions"
        )),
        several => Err(format!(
            "{query:?} matches {} sessions: {}. Use more of its id",
            several.len(),
            several
                .iter()
                .map(|index| format!(
                    "{} ({})",
                    candidates[*index].1,
                    short_id(candidates[*index].0)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_picked_by_its_id_its_title_or_a_long_enough_part_of_it() {
        let sessions = [
            ("9fb6fe92-aaaa", "Tailscale machine monitoring"),
            ("07f37932-bbbb", "Flick Pong MVP spec"),
            ("07f3aaaa-cccc", "fix the machine list"),
        ];
        assert_eq!(pick(&sessions, "9fb6"), Ok(0));
        assert_eq!(pick(&sessions, "flick pong mvp spec"), Ok(1));
        assert_eq!(pick(&sessions, "pong"), Ok(1));
        assert!(
            pick(&sessions, "ma")
                .unwrap_err()
                .contains("no session matches")
        );
        assert!(
            pick(&sessions, "9fb")
                .unwrap_err()
                .contains("no session matches")
        );
        let several = pick(&sessions, "machine").unwrap_err();
        assert!(several.contains("matches 2 sessions"), "{several}");
        assert!(
            pick(&sessions, "07f3")
                .unwrap_err()
                .contains("matches 2 sessions")
        );
    }
}
