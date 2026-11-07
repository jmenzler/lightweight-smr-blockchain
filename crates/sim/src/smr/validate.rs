use super::types::MAX_APPEND_REQUESTS;
use super::{AUTO_CLIENT_BASE, AUTO_OP_BASE, SmrScenario, TrafficPhase};

impl SmrScenario {
    /// Paper preconditions and v1 restrictions, checked before any run.
    pub fn validate(&self) -> Result<(), String> {
        if self.n < 1 {
            return Err("n must be at least 1".into());
        }
        self.cfg.check().map_err(|e| e.to_string())?;
        self.schedule.validate(self.n)?;
        if crate::blocking::schedule_knowledge(&self.schedule, self.n).reads_node_state() {
            return Err(
                "adaptive schedules are Alg-1 only: the SMR mask path has no node-state hook"
                    .into(),
            );
        }
        if self.proto == super::Proto::Extended && !self.merge_policy.is_default() {
            return Err("merge_policy applies only to compact and recovery nodes".into());
        }
        if self.repeated_commit != super::RepeatedCommit::Execute {
            if self.proto == super::Proto::Extended {
                return Err("repeated_commit applies only to compact and recovery nodes".into());
            }
            if self.certs && matches!(self.proto, super::Proto::Recovery { .. }) {
                return Err(
                    "repeated_commit skip is not combined with the §5 certificate layer".into(),
                );
            }
        }
        if !(self.sigma >= 1.0 && self.sigma.is_finite()) {
            return Err(format!(
                "sigma must be finite and at least 1 (Alg 3 B1, p. 20), got {}",
                self.sigma
            ));
        }
        if self.sigma * (self.n as f64).log2() > MAX_APPEND_REQUESTS {
            return Err(format!(
                "sigma {} sends more than {MAX_APPEND_REQUESTS} append requests per delivery at n={}",
                self.sigma, self.n
            ));
        }
        if matches!(self.proto, super::Proto::Compact { t_commit_rounds: 0 }) {
            return Err("compact t_commit_rounds must be at least 1".into());
        }
        if matches!(
            self.proto,
            super::Proto::Recovery {
                t_window_rounds: 0,
                ..
            }
        ) {
            return Err("recovery t_window_rounds must be at least 1".into());
        }
        let mut ops = std::collections::BTreeSet::new();
        for inj in &self.injections {
            if inj.round < 1 {
                return Err("injection rounds are 1-based".into());
            }
            validate_injection(inj.client, inj.op, inj.target, self.n, !ops.insert(inj.op))?;
        }
        if let Some(phases) = &self.traffic {
            validate_traffic(phases)?;
        }
        if let Some(clients) = self.client_model.pool_size() {
            if clients < 1 {
                return Err("client_model pool needs at least one client".into());
            }
            if clients > AUTO_CLIENT_BASE {
                return Err(format!(
                    "client_model pool of {clients} clients leaves the auto namespace (> {AUTO_CLIENT_BASE})"
                ));
            }
            // Exhaustive: a new rule must fail to compile here until shown pool-compatible.
            match self.proto {
                crate::smr::Proto::Compact { .. } | crate::smr::Proto::Recovery { .. } => {}
                crate::smr::Proto::Extended => {
                    return Err(
                        "client_model pool is compact/recovery only: the extended rule has no \
                         commitment latch to free a client on"
                            .into(),
                    );
                }
            }
        }
        crate::check_partition(self.partition.as_deref(), self.n)?;
        for mb in &self.manual_blocks {
            if mb.node as usize >= self.n {
                return Err(format!(
                    "manual block node {} out of range for n={}",
                    mb.node, self.n
                ));
            }
            if mb.from_round < 1 {
                return Err("manual block rounds are 1-based".into());
            }
            if let Some(to) = mb.to_round
                && to <= mb.from_round
            {
                return Err(format!(
                    "manual block to_round {to} must be after from_round {}",
                    mb.from_round
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn validate_injection(
    client: u32,
    op: u64,
    target: Option<u32>,
    n: usize,
    duplicate: bool,
) -> Result<(), String> {
    if op == 0 {
        return Err("op 0 is the reserved seed command x0".into());
    }
    if client >= AUTO_CLIENT_BASE {
        return Err(format!(
            "client {client} is in the auto-arrival namespace (>= {AUTO_CLIENT_BASE})"
        ));
    }
    if op >= AUTO_OP_BASE {
        return Err(format!(
            "op {op} is in the auto-arrival namespace (>= {AUTO_OP_BASE})"
        ));
    }
    if duplicate {
        return Err(format!("duplicate op {op}"));
    }
    if let Some(t) = target
        && t as usize >= n
    {
        return Err(format!("target {t} out of range for n={n}"));
    }
    Ok(())
}

fn validate_traffic(phases: &[TrafficPhase]) -> Result<(), String> {
    if phases.is_empty() {
        return Err("traffic needs at least one phase".into());
    }
    let mut prev = 0;
    for p in phases {
        if p.from_round < 1 {
            return Err("phase rounds are 1-based".into());
        }
        if p.from_round <= prev {
            return Err(format!(
                "phase from_round {} not after previous {prev}",
                p.from_round
            ));
        }
        prev = p.from_round;
        validate_pmf(&p.arrivals_pmf)?;
    }
    Ok(())
}

pub(super) fn validate_pmf(weights: &[f64]) -> Result<(), String> {
    validate_weights("arrivals pmf", "pmf weights", weights)
}

pub(crate) fn validate_weights(subject: &str, noun: &str, weights: &[f64]) -> Result<(), String> {
    if weights.is_empty() {
        return Err(format!("{subject} needs at least one weight"));
    }
    if let Some(bad) = weights.iter().find(|w| !w.is_finite() || **w < 0.0) {
        return Err(format!("{noun} must be finite and non-negative, got {bad}"));
    }
    if weights.iter().sum::<f64>() <= 0.0 {
        return Err(format!("{noun} must not all be zero"));
    }
    Ok(())
}

/// Traffic pmf with all mass at `rate` arrivals per round.
pub fn point_mass_pmf(rate: usize) -> Vec<f64> {
    let mut pmf = vec![0.0; rate + 1];
    pmf[rate] = 1.0;
    pmf
}
