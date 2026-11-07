//! Split-brain persistence probe. Usage: rq2_persistence <probe-spec.json> [out.csv]

use sim::persistence::{PROBE_CSV_HEADER, detail_path, parse_probe_specs, probe_row, probe_run};
use sim::runlog::{ledger_target, log_smr_run_to};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let spec_path = args
        .get(1)
        .expect("usage: rq2_persistence <probe-spec.json> [out.csv]");
    let spec_text = std::fs::read_to_string(spec_path).expect("read spec");
    let specs = parse_probe_specs(&spec_text).unwrap_or_else(|e| panic!("{e}"));
    assert!(!specs.is_empty(), "spec file holds no cells");

    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("{}-probe.csv", specs[0].exp));
    assert_ne!(&out, spec_path, "the output would overwrite the spec");
    let partial = format!("{out}.partial");
    let _ = std::fs::remove_file(&partial);
    let ledger = ledger_target(None);

    let mut csv = String::from(PROBE_CSV_HEADER);
    csv.push('\n');
    let mut records: Vec<serde_json::Value> = Vec::new();

    for spec in &specs {
        let runs = spec.expand().unwrap_or_else(|e| panic!("{e}"));
        let (mut split, mut healed, mut mismatched) = (0usize, 0usize, 0usize);
        let mut retracting = 0usize;
        for (scenario, opts) in &runs {
            let probe = probe_run(scenario, opts).unwrap_or_else(|e| panic!("{e}"));
            csv.push_str(&probe_row(&spec.exp, scenario, &probe));
            csv.push('\n');
            std::fs::write(&partial, &csv).expect("write partial");

            if let Some(path) = &ledger {
                let extra = serde_json::json!({
                    "exp": spec.exp,
                    "r_v": probe.r_v,
                    "truncated": probe.truncated,
                    "replay_ok": probe.replay_ok,
                });
                log_smr_run_to(path, scenario, &probe.report, Some(extra))
                    .expect("ledger write failed (set SIM_RUNLOG=off to opt out)");
            }

            split += usize::from(probe.r_v.is_some());
            healed += usize::from(probe.transience.as_ref().is_some_and(|t| t.r_h.is_some()));
            mismatched += usize::from(probe.replay_ok == Some(false));
            retracting += usize::from(probe.report.retractions > 0);
            records.push(serde_json::json!({
                "exp": spec.exp,
                "scenario": scenario,
                "probe": probe,
            }));
        }
        eprintln!(
            "{}: n={} T={:?} {}/{} split, {healed} back to consistency (by retracting executed \
             commands), {retracting} with a Def 1.5 retraction, {mismatched} replay mismatches",
            spec.exp,
            spec.base.n,
            spec.base.proto,
            split,
            runs.len(),
        );
    }

    std::fs::write(&out, &csv).expect("write csv");
    let detail = detail_path(&out);
    assert_ne!(
        &detail, spec_path,
        "the detail file would overwrite the spec"
    );
    std::fs::write(
        &detail,
        serde_json::to_string(&records).expect("records serialize"),
    )
    .expect("write detail json");
    let _ = std::fs::remove_file(&partial);
    let _ = std::io::stderr().flush();
    eprintln!("wrote {out} and {detail}");
}
