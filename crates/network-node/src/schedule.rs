//! Deterministic injection schedules from a dedicated "schedule" RNG stream.

use crate::rng::derive_rng;
use rand::Rng;
use serde::Serialize;
use sim::draw_weighted_index;
use sim::smr::{AUTO_CLIENT_BASE, AUTO_OP_BASE};
use sim::spec::InjectionSpec;

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub seed: u64,
    pub pmf: Vec<f64>,
    pub rounds: u64,
    pub clients: u32,
    pub per_client: u32,
    pub n: u32,
    pub pin_targets: bool,
}

/// Places `clients * per_client` commands; draw order per round: arrival count, then one target draw per
/// command. The draw is taken even when unpinned, so pinning never moves the arrivals; an unpinned command
/// (the default) is sent to a fresh uniform server on every attempt.
pub fn generate(
    seed: u64,
    rounds: u64,
    pmf: &[f64],
    clients: u32,
    per_client: u32,
    n: u32,
    pin_targets: bool,
) -> Result<Vec<InjectionSpec>, String> {
    if pmf.is_empty() {
        return Err("pmf needs at least one weight".into());
    }
    if let Some(bad) = pmf.iter().find(|w| !w.is_finite() || **w < 0.0) {
        return Err(format!(
            "pmf weights must be finite and non-negative, got {bad}"
        ));
    }
    if n == 0 {
        return Err("n must be at least 1 (target draws need a nonempty range)".into());
    }
    let clients_u64 = clients as u64;
    if clients_u64 >= AUTO_CLIENT_BASE as u64 {
        return Err(format!(
            "clients {clients} would assign ids reaching the auto-arrival namespace (>= {AUTO_CLIENT_BASE})"
        ));
    }
    let total = clients_u64 * per_client as u64;
    if total >= AUTO_OP_BASE {
        return Err(format!(
            "clients*per_client {total} would assign ops reaching the auto-arrival namespace (>= {AUTO_OP_BASE})"
        ));
    }

    let mut rng = derive_rng(seed, "schedule", 0);
    let mut out = Vec::with_capacity(total as usize);
    let mut op = 1u64;
    'rounds: for round in 1..=rounds {
        if out.len() as u64 >= total {
            break;
        }
        let arrivals = draw_weighted_index(&mut rng, pmf);
        for _ in 0..arrivals {
            if out.len() as u64 >= total {
                break 'rounds;
            }
            let client = 1 + (out.len() as u64 % clients_u64) as u32;
            let target = rng.random_range(0..n as usize) as u32;
            out.push(InjectionSpec {
                round: round as usize,
                client,
                op,
                target: pin_targets.then_some(target),
            });
            op += 1;
        }
    }
    if (out.len() as u64) < total {
        return Err(format!(
            "horizon of {rounds} rounds only placed {} of {total} commands",
            out.len()
        ));
    }
    Ok(out)
}
