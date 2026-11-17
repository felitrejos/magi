use std::collections::{HashMap, HashSet};

use crate::proto::Tokens;

pub struct Counted {
    pub tokens: Tokens,
    pub cost: u64,
    pub priced: bool,
}

#[derive(Default)]
pub struct Ledger {
    counted: HashMap<String, Counted>,
    copied: HashMap<String, Counted>,
    pub tokens: Tokens,
    pub cost_micros: u64,
    pub unpriced: bool,
}

impl Ledger {
    pub fn count(&mut self, id: &str, seen: Counted, claims: &mut HashSet<String>) {
        if let Some(copy) = self.copied.get_mut(id) {
            copy.keep_larger(&seen);
            return;
        }
        match self.counted.get_mut(id) {
            Some(counted) => {
                let before = (counted.tokens.output, counted.cost);
                counted.keep_larger(&seen);
                self.tokens.output += counted.tokens.output - before.0;
                self.cost_micros += counted.cost - before.1;
                self.unpriced |= !counted.priced;
            }
            None if claims.insert(id.to_string()) => {
                self.add(&seen);
                self.counted.insert(id.to_string(), seen);
            }
            None => {
                self.copied.insert(id.to_string(), seen);
            }
        }
    }

    pub fn release(&self, claims: &mut HashSet<String>) {
        for id in self.counted.keys() {
            claims.remove(id);
        }
    }

    pub fn adopt(&mut self, claims: &mut HashSet<String>) {
        for (id, counted) in std::mem::take(&mut self.copied) {
            if claims.insert(id.clone()) {
                self.add(&counted);
                self.counted.insert(id, counted);
            } else {
                self.copied.insert(id, counted);
            }
        }
    }

    fn add(&mut self, counted: &Counted) {
        self.tokens = self.tokens + counted.tokens;
        self.cost_micros += counted.cost;
        self.unpriced |= !counted.priced;
    }
}

impl Counted {
    fn keep_larger(&mut self, seen: &Counted) {
        if seen.tokens.output > self.tokens.output {
            self.tokens.output = seen.tokens.output;
            self.cost = self.cost.max(seen.cost);
            self.priced &= seen.priced;
        }
    }
}
