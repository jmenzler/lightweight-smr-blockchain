//! A compact or recovery live session dates T_E (`prefix_fixed_round`) over each server's
//! executed part ++ log; batch runs never do. Everything else must replay byte-identically.

use serde_json::Value;
use sim_wasm::LiveSmr;

/// `report` with the live-only landmark cleared when `proto_kind` is not extended.
pub fn without_live_only(mut report: Value, proto_kind: &str) -> Value {
    if proto_kind != "extended" {
        for c in report["commands"].as_array_mut().expect("commands") {
            c["prefix_fixed_round"] = Value::Null;
        }
    }
    report
}

/// The live report as its batch replay must reproduce it.
pub fn live_report_as_batch(live: &LiveSmr) -> Value {
    let scenario: Value = serde_json::from_str(&live.export_scenario()).expect("scenario json");
    let kind = scenario["proto"]["kind"].as_str().expect("proto kind");
    without_live_only(
        serde_json::from_str(&live.report_json()).expect("live report json"),
        kind,
    )
}
