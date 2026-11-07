//! Digests pin the draw stream; derived metric columns are removed before hashing.

/// Observer-only members: the useful-executed spread, the Def 1.4 / Def 1.5 oracle counters and
/// per-row verdict, recovery's commit-path checkpoint-fork date, the Def 1.3 index-agreement
/// landmark and the injection landmark.
const DERIVED: [&str; 12] = [
    "useful_executed",
    "retractions",
    "retraction_rewrites",
    "first_retraction_round",
    "strong_safety_violation_rounds",
    "strong_safety_first_violation",
    "strong_safety_boundary_violations",
    "strong_safety_boundary_first_violation",
    "commit_fork_round",
    "index_agreed_round",
    "injected_round",
    "useful_agree",
];

/// Removes every derived member (`"key":<null|bool|number|[lo,hi]>`) from a serialized report or status.
pub fn without_derived_fields(json: &str) -> String {
    DERIVED
        .iter()
        .fold(json.to_string(), |acc, key| without_member(&acc, key))
}

fn without_member(json: &str, key: &str) -> String {
    let key = format!("\"{key}\":");
    let mut out = String::with_capacity(json.len());
    let mut rest = json;
    while let Some(at) = rest.find(&key) {
        let value = &rest[at + key.len()..];
        let end = if value.starts_with('[') {
            value.find(']').expect("array closes") + 1
        } else {
            value
                .find([',', '}'])
                .expect("a member ends at a comma or a brace")
        };
        if value[end..].starts_with(',') {
            out.push_str(&rest[..at]);
            rest = &value[end + 1..];
        } else {
            // The last member: drop the comma before it instead.
            out.push_str(rest[..at].strip_suffix(',').unwrap_or(&rest[..at]));
            rest = &value[end..];
        }
    }
    out.push_str(rest);
    out
}
