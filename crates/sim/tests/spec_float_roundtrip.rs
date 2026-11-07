//! A fraction printed with 17 significant digits must parse back to the same f64, in whichever
//! crate set is built, so a recorded spec always yields the blocked counts it was recorded with.

use sim::spec::{ScenarioSpec, ScheduleSpec};

#[test]
fn a_long_decimal_fraction_parses_to_the_exact_double() {
    let fraction: f64 = 2.0 / 13.0;
    let text = serde_json::to_string(&fraction).unwrap();
    assert_eq!(text, "0.15384615384615385");
    assert_eq!(
        serde_json::from_str::<f64>(&text).unwrap().to_bits(),
        fraction.to_bits()
    );
}

#[test]
fn a_per_round_fraction_in_a_spec_keeps_its_bits() {
    let fraction: f64 = 2.0 / 13.0;
    let json = format!(
        r#"{{"n":13,"seed":4,"k":6,"ell":3,"init":{{"kind":"split","fraction":0.5}},"max_rounds":1,
            "schedule":{{"kind":"per_round_fractions","fractions":[{fraction}]}}}}"#
    );
    let spec: ScenarioSpec = serde_json::from_str(&json).unwrap();
    let Some(ScheduleSpec::PerRoundFractions { fractions }) = spec.schedule else {
        panic!("per_round_fractions schedule expected");
    };
    assert_eq!(fractions[0].to_bits(), fraction.to_bits());
}
