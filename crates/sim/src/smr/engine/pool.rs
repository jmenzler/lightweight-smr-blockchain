//! Bounded client pool: K reusable clients, one command in flight each, monotone sns.

use crate::smr::AUTO_CLIENT_BASE;
use rand::Rng;
use rand_chacha::ChaCha12Rng;

/// Start of the exhaustion panic message; the grid runner matches on it to record an abort row.
pub(crate) const EXHAUSTED_MARKER: &str = "client pool exhausted";

pub(in crate::smr) struct ClientPool {
    // Sorted: the uniform draw indexes this list, so its order is part of the RNG contract.
    idle: Vec<u32>,
    next_sn: Vec<u64>,
    peak_in_flight: u32,
}

impl ClientPool {
    pub(in crate::smr) fn new(clients: u32) -> ClientPool {
        ClientPool {
            idle: (0..clients).collect(),
            next_sn: vec![1; clients as usize],
            peak_in_flight: 0,
        }
    }

    /// Assign an arrival to a uniformly drawn idle client: exactly one RNG draw, pool path only.
    pub(in crate::smr) fn take(&mut self, rng: &mut ChaCha12Rng, round: usize) -> (u32, u64) {
        assert!(
            !self.idle.is_empty(),
            "{EXHAUSTED_MARKER} in round {round}: all {} clients are in flight — \
             the pool is undersized for this arrival rate",
            self.next_sn.len()
        );
        let index = self.idle.remove(rng.random_range(0..self.idle.len()));
        let next = &mut self.next_sn[index as usize];
        let sn = *next;
        *next += 1;
        let in_flight = (self.next_sn.len() - self.idle.len()) as u32;
        self.peak_in_flight = self.peak_in_flight.max(in_flight);
        (AUTO_CLIENT_BASE + index, sn)
    }

    pub(in crate::smr) fn free(&mut self, client: u32) {
        let Some(index) = client.checked_sub(AUTO_CLIENT_BASE) else {
            return;
        };
        assert!(
            (index as usize) < self.next_sn.len(),
            "pool client {index} was never issued by this pool of {}",
            self.next_sn.len()
        );
        match self.idle.binary_search(&index) {
            Ok(_) => panic!("pool client {index} released twice"),
            Err(at) => self.idle.insert(at, index),
        }
    }

    pub(in crate::smr) fn peak_in_flight(&self) -> u32 {
        self.peak_in_flight
    }
}

#[cfg(test)]
mod tests;
