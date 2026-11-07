use sim::spec::ScheduleSpec;

pub(crate) fn error_json(error: impl std::fmt::Display) -> String {
    serde_json::json!({ "error": error.to_string() }).to_string()
}

/// Def 1.1 fixes beta in [0, 1]; the session must record only values the replay spec accepts.
pub(crate) fn check_fraction(fraction: f64) -> Result<(), String> {
    if (0.0..=1.0).contains(&fraction) {
        Ok(())
    } else {
        Err(format!(
            "blocking fraction must be within [0, 1], got {fraction}"
        ))
    }
}

pub(crate) fn recorded_schedule(sticky: bool, fractions: &[f64], background: f64) -> ScheduleSpec {
    let fractions = fractions.to_vec();
    if sticky {
        ScheduleSpec::PerRoundSticky {
            fractions,
            background,
        }
    } else {
        ScheduleSpec::PerRoundFractions { fractions }
    }
}

pub(crate) fn hex(h: &protocol::certificates::Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn peaks_json(forest: &protocol::certificates::MmrForest) -> Vec<serde_json::Value> {
    forest
        .peaks()
        .iter()
        .map(|(height, root, start)| {
            serde_json::json!({
                "height": height,
                "root_hex": hex(root),
                "start": start,
            })
        })
        .collect()
}

pub(crate) fn merge_json(e: &protocol::certificates::MergeEvent) -> serde_json::Value {
    serde_json::json!({
        "height": e.height,
        "start": e.start,
        "root_hex": hex(&e.parent),
    })
}

pub(crate) fn chain_snapshot(entries: &[protocol::compact::Entry], from_len: u64) -> String {
    let from = (from_len as usize).min(entries.len());
    let mut forest = protocol::certificates::MmrForest::new();
    for entry in &entries[..from] {
        forest.append(protocol::certificates::leaf_hash(entry));
    }
    let merges: Vec<serde_json::Value> = entries[from..]
        .iter()
        .flat_map(|entry| forest.append(protocol::certificates::leaf_hash(entry)))
        .map(|e| merge_json(&e))
        .collect();
    let peaks = peaks_json(&forest);
    let delta: Vec<serde_json::Value> = entries[from..]
        .iter()
        .map(|entry| match entry {
            protocol::compact::Entry::Cmd(cc) => serde_json::json!({
                "kind": "cmd", "client": cc.client, "sn": cc.sn, "op": cc.op
            }),
            protocol::compact::Entry::Null { client, sn } => {
                serde_json::json!({ "kind": "null", "client": client, "sn": sn })
            }
            protocol::compact::Entry::Nop(op) => {
                serde_json::json!({ "kind": "nop", "op": op })
            }
        })
        .collect();
    serde_json::json!({
        "m": entries.len(),
        "peaks": peaks,
        "merges": merges,
        "entries": delta,
    })
    .to_string()
}
