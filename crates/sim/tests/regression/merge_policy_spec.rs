//! Merge policy (stamp tie + union stamp) as a recorded spec field.

use sim::smr::{SmrScenario, run_smr};
use sim::spec::SmrScenarioSpec;

use crate::digest_fields;

const STICKY: &str = r#"{"kind":"per_round_sticky","fractions":[0,0,0,0,0,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6,0.6],"background":0.1}"#;
const COMPACT: &str = r#"{"kind":"compact","t_commit_rounds":10}"#;
const RECOVERY: &str = r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#;

fn spec_json(proto: &str, policy: Option<&str>) -> String {
    let policy = policy
        .map(|p| format!(r#","merge_policy":{p}"#))
        .unwrap_or_default();
    format!(
        r#"{{"n":32,"seed":1,"k":6,"ell":3,"sigma":1.0,"proto":{proto},
        "injections":[],"max_rounds":80,"schedule":{STICKY},
        "traffic":{{"kind":"pmf","arrivals_pmf":[0.0,0.0,1.0]}}{policy}}}"#
    )
}

fn scenario(json: &str) -> SmrScenario {
    let spec: SmrScenarioSpec = serde_json::from_str(json).expect("parses");
    SmrScenario::try_from(spec).expect("validates")
}

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn report_digest(json: &str) -> u64 {
    let report = run_smr(&scenario(json));
    fnv64(
        digest_fields::without_derived_fields(&serde_json::to_string(&report).expect("serializes"))
            .as_bytes(),
    )
}

/// Goldens captured through the retired SIM_STAMP_TIE / SIM_UNION_STAMP env path; the recovery
/// digests differ from those only in resends of an x already in L_i, which Alg 5 reports as ignored,
/// and in `wedged_round`: this recovery cell's fleet is log-less on the genesis checkpoint from round 7.
/// The `last_sighting` digests changed again once that policy stopped re-stamping entries of the
/// median L'_i; the `first_sighting` ones did not move.
#[test]
fn spec_policy_reproduces_the_env_path_goldens() {
    let cases = [
        (None, 0xf6a4_85b3_4a00_9b5a, 0x9374_1f39_3dcc_f9d3),
        (
            Some(r#"{"stamp_tie":"commands_only","union_stamp":"first_sighting"}"#),
            0x1bb5_f45b_ed6a_ec11,
            0x7ecf_3ad7_0ff0_ef54,
        ),
        (
            Some(r#"{"stamp_tie":"include_round","union_stamp":"last_sighting"}"#),
            0xb76d_d0a9_c85b_d2c7,
            0x411a_775b_b7d3_a4c6,
        ),
        (
            Some(r#"{"stamp_tie":"commands_only","union_stamp":"last_sighting"}"#),
            0xb045_5df6_7728_86a7,
            0xcea8_228d_4033_3272,
        ),
    ];
    for (policy, compact, recovery) in cases {
        assert_eq!(
            report_digest(&spec_json(COMPACT, policy)),
            compact,
            "compact {policy:?}"
        );
        assert_eq!(
            report_digest(&spec_json(RECOVERY, policy)),
            recovery,
            "recovery {policy:?}"
        );
    }
}

#[test]
fn default_policy_stays_off_the_wire() {
    let plain = spec_json(COMPACT, None);
    let spec: SmrScenarioSpec = serde_json::from_str(&plain).expect("parses");
    let spec_wire = serde_json::to_string(&spec).expect("serializes");
    assert!(!spec_wire.contains("merge_policy"), "{spec_wire}");
    let scenario_wire = serde_json::to_string(&scenario(&plain)).expect("serializes");
    assert!(!scenario_wire.contains("merge_policy"), "{scenario_wire}");

    let explicit = spec_json(
        COMPACT,
        Some(r#"{"stamp_tie":"include_round","union_stamp":"first_sighting"}"#),
    );
    assert_eq!(
        serde_json::to_string(&scenario(&explicit)).expect("serializes"),
        scenario_wire
    );
    let partial = spec_json(COMPACT, Some(r#"{"union_stamp":"first_sighting"}"#));
    assert_eq!(
        serde_json::to_string(&scenario(&partial)).expect("serializes"),
        scenario_wire
    );
}

#[test]
fn non_default_policy_is_recorded_in_the_scenario() {
    let json = spec_json(
        RECOVERY,
        Some(r#"{"stamp_tie":"commands_only","union_stamp":"last_sighting"}"#),
    );
    let wire = serde_json::to_string(&scenario(&json)).expect("serializes");
    assert!(
        wire.contains(
            r#""merge_policy":{"stamp_tie":"commands_only","union_stamp":"last_sighting"}"#
        ),
        "{wire}"
    );
}

#[test]
fn extended_proto_rejects_a_merge_policy_it_cannot_apply() {
    let json = spec_json(
        r#"{"kind":"extended"}"#,
        Some(r#"{"stamp_tie":"commands_only"}"#),
    );
    let spec: SmrScenarioSpec = serde_json::from_str(&json).expect("parses");
    assert!(SmrScenario::try_from(spec).is_err());
}
