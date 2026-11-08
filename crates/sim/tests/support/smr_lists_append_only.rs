//! A command's spread curve and delivery attempts only grow at the end between two reports of
//! one session: the live lab's delta cursor sends only their tails, so an interior edit here
//! would leave the page silently stale.

use sim::smr::{CommandReport, DeliveryAttempt, SmrReport, SmrScenario, SmrState, SpreadPoint};
use sim::spec::SmrScenarioSpec;
use std::collections::HashMap;

fn state(proto: &str, client_model: &str, seed: u64) -> SmrState {
    let spec: SmrScenarioSpec = serde_json::from_str(&format!(
        r#"{{"n":32,"seed":{seed},"k":6,"ell":3,"sigma":1.5,"proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}}],"max_rounds":400,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}},
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.2,0.4,0.4]}}{client_model}}}"#
    ))
    .unwrap();
    let s = SmrScenario::try_from(spec).unwrap();
    let mut state = SmrState::new_with_certs(
        s.n,
        s.cfg,
        s.proto,
        s.sigma,
        s.seed,
        &s.injections,
        s.traffic.as_deref().unwrap_or(&[]),
        s.client_model,
        None,
        true,
        s.merge_policy,
        s.repeated_commit,
    );
    if !matches!(s.proto, sim::smr::Proto::Extended) {
        state.observe_full_prefix();
    }
    state
}

type Lists = (Vec<SpreadPoint>, Vec<DeliveryAttempt>);

fn lists(report: &SmrReport) -> HashMap<u64, Lists> {
    report
        .commands
        .iter()
        .map(|c: &CommandReport| {
            let spread = c.spread.observed().expect("full scope").to_vec();
            (c.op, (spread, c.attempts.clone()))
        })
        .collect()
}

#[test]
fn spread_and_attempts_only_grow_at_the_end() {
    let protos = [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":2}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
        r#"{"kind":"recovery","t_window_rounds":4,"resend_until_acked":true}"#,
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
    ];
    let models = ["", r#","client_model":{"kind":"pool","clients":256}"#];
    let mut checked = 0usize;
    for proto in protos {
        for model in models {
            if !model.is_empty() && proto.contains("extended") {
                continue;
            }
            for seed in [1, 5] {
                let mut state = state(proto, model, seed);
                let mut before = lists(&state.report());
                for round in 1..=160 {
                    state.draw_arrivals();
                    // 10 of every 40 rounds block 60%: surges, rollbacks and re-sends
                    let fraction = if round % 40 >= 30 { 0.6 } else { 0.1 };
                    let count = state.blocked_count(fraction);
                    let mask = state.sample_mask(count);
                    let status = state.step_masked(&mask);
                    let ended = status.failed() || status.dead.is_some();
                    let now = lists(&state.report());
                    for (op, (spread, attempts)) in &before {
                        let (s, a) = now
                            .get(op)
                            .unwrap_or_else(|| panic!("{proto}{model} r{round}: op {op} vanished"));
                        assert!(
                            s.starts_with(spread) && a.starts_with(attempts),
                            "{proto}{model} seed {seed} r{round}: op {op} rewrote a list"
                        );
                        checked += 1;
                    }
                    before = now;
                    if ended {
                        break;
                    }
                }
            }
        }
    }
    assert!(checked > 10_000, "too few comparisons: {checked}");
}
