use std::io;
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;

use super::Event;
use super::model::Topology;
use crate::tailscale;

const EVERY: Duration = Duration::from_secs(5);

pub fn spawn(events: SyncSender<Event>) -> io::Result<()> {
    thread::Builder::new()
        .name("tailscale".into())
        .spawn(move || watch(&events))?;
    Ok(())
}

fn watch(events: &SyncSender<Event>) {
    let mut topology = Topology::default();
    let mut published = None;
    loop {
        match tailscale::status() {
            Ok(status) => {
                topology.self_node = Some(status.node);
                topology.peers = status.peers;
                topology.problem = None;
            }
            Err(error) => topology.problem = Some(format!("tailscale: {error}")),
        }
        if published.as_ref() != Some(&topology) {
            if events.send(Event::Topology(topology.clone())).is_err() {
                return;
            }
            published = Some(topology.clone());
        }
        thread::sleep(EVERY);
    }
}
