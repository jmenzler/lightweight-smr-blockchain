//! Byte-identity tripwire for everything a sweep grid emits: each CSV row in full, each ledger
//! record (provenance stripped, spill path replaced by the spill's digest) and each canonical
//! spill body. Cells cover compact and recovery, unique and pooled clients, β = 0 and 0.1, the
//! ack and the executed client, §5 certificates, and ran / dead / halted / abort rows.
//! Regenerate only as a deliberate, commit-noted act: `SWEEP_GOLDEN_BLESS=1` rewrites the fixture.
//! A single-file binary because it sets the process-global ledger and spill switches.

use sim::sweep::{AnySpec, parse_specs, run_smr_grid_streaming};
use std::path::{Path, PathBuf};

const FIXTURE: &str = "tests/fixtures/sweep_output_golden.txt";

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn base(proto: &str, n: usize, max_rounds: usize, extra: &str) -> String {
    format!(
        r#"{{"n":{n},"seed":0,"k":6,"ell":3,"sigma":1.0,"proto":{proto},"injections":[],
            "max_rounds":{max_rounds},"schedule":{{"kind":"fresh_per_round","fraction":0.0}}{extra}}}"#
    )
}

fn grid(exp: &str, base: &str, axes: &str, n: usize, seed_base: u64, seeds: u64) -> String {
    format!(
        r#"{{"exp":"{exp}","base":{base},{axes}"ladder":[{{"n":{n},"seed_base":{seed_base},"seed_count":{seeds}}}]}}"#
    )
}

const COMPACT: &str = r#"{"kind":"compact","t_commit_rounds":21}"#;
const RECOVERY: &str = r#"{"kind":"recovery","t_window_rounds":20,"resend_until_acked":false}"#;
const RECOVERY_ACK: &str = r#"{"kind":"recovery","t_window_rounds":20,"resend_until_acked":true}"#;
const LOADED: &str = r#""betas":[0.0,0.1],"rates":[2,6],"#;

#[rustfmt::skip]
fn specs() -> Vec<String> {
    let pool = |k: u32| format!(r#","client_model":{{"kind":"pool","clients":{k}}}"#);
    let surge: String = (0..32)
        .map(|node| format!(r#"{{"node":{node},"from_round":37,"to_round":38}}"#))
        .collect::<Vec<_>>()
        .join(",");
    vec![
        grid("comp-unique", &base(COMPACT, 32, 400, ""), LOADED, 32, 7100, 2),
        grid("comp-pool", &base(COMPACT, 32, 400, &pool(400)), LOADED, 32, 7200, 2),
        grid("comp-pool-tight", &base(COMPACT, 32, 400, &pool(48)), r#""rates":[6],"#, 32, 7250, 1),
        grid("rec-unique", &base(RECOVERY, 32, 400, ""), LOADED, 32, 7300, 2),
        grid("rec-pool-ack-certs", &base(RECOVERY_ACK, 32, 400, &(pool(400) + r#","certs":true"#)), LOADED, 32, 7400, 2),
        grid("rec-pool", &base(RECOVERY, 32, 400, &pool(400)), LOADED, 32, 7350, 2),
        grid("rec-pool-ack", &base(RECOVERY_ACK, 32, 400, &pool(400)), r#""betas":[0.1],"rates":[2,4],"#, 32, 7450, 2),
        grid("rec-mixed", &base(RECOVERY, 32, 300, ""), r#""betas":[0.12,0.15],"rates":[4],"#, 32, 7650, 2),
        grid("rec-unique-certs", &base(RECOVERY, 32, 400, r#","certs":true"#), r#""betas":[0.1],"rates":[4],"#, 32, 7500, 2),
        grid("rec-ack-unique", &base(RECOVERY_ACK, 32, 400, ""), r#""betas":[0.1],"rates":[4],"#, 32, 7550, 2),
        grid("rec-short", &base(RECOVERY, 32, 90, ""), r#""betas":[0.3],"rates":[6],"#, 32, 7600, 3),
        grid("rec-surge", &base(RECOVERY, 32, 140, &format!(r#","manual_blocks":[{surge}]"#)), r#""betas":[0.1],"rates":[4],"#, 32, 7700, 4),
        grid("comp-split", &base(COMPACT, 64, 400, ""), r#""rates":[4],"#, 64, 950_000, 1),
        grid("comp-split-halt", &base(COMPACT, 64, 400, r#","halt_on_violation":true"#), r#""rates":[4],"#, 64, 950_000, 1),
        grid("comp-dead", &base(COMPACT, 16, 400, ""), r#""betas":[0.45,0.6],"rates":[2],"#, 16, 7800, 3),
    ]
}

fn spill_digest(path: &str) -> u64 {
    let text = std::fs::read_to_string(path).expect("spill readable");
    std::fs::remove_file(path).expect("spill removable");
    let (header, body) = text.split_once('\n').expect("spill has a header line");
    assert!(header.contains("\"kind\":\"header\""), "{header}");
    fnv(body.as_bytes())
}

const PROVENANCE: [&str; 5] = ["timestamp", "commit", "dirty", "cwd_commit", "cwd_dirty"];

/// One ledger record with its provenance dropped and its spill path replaced by the body digest.
fn normalized_record(line: &str) -> String {
    let mut record: serde_json::Value = serde_json::from_str(line).expect("ledger line is JSON");
    let map = record.as_object_mut().expect("ledger record is an object");
    for key in PROVENANCE {
        map.remove(key);
    }
    if let Some(path) = map.remove("spill_path") {
        let path = path.as_str().expect("spill path is a string").to_string();
        map.insert(
            "spill_fnv".into(),
            format!("{:016x}", spill_digest(&path)).into(),
        );
    }
    record.to_string()
}

fn run_all(ledger: &Path) -> String {
    let mut out = String::new();
    for json in specs() {
        let mut parsed = parse_specs(&json).expect("valid spec");
        assert_eq!(parsed.len(), 1, "one spec");
        let AnySpec::Smr(spec) = parsed.remove(0) else {
            panic!("an SMR grid spec");
        };
        let csv = run_smr_grid_streaming(&spec, Some(ledger), None).expect("grid ran");
        out.push_str(&csv);
    }
    out
}

fn ledger_lines(ledger: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(ledger).expect("ledger readable");
    text.lines().map(normalized_record).collect()
}

#[test]
fn sweep_outputs_are_byte_identical() {
    // SAFETY: the only test in this binary, before any sim code reads the environment.
    unsafe {
        std::env::remove_var("SIM_SPILL");
        std::env::remove_var("SIM_RUNLOG");
    }
    let dir = std::env::temp_dir().join(format!("sweep-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let ledger: PathBuf = dir.join("ledger.jsonl");
    let _ = std::fs::remove_file(&ledger);

    let csv = run_all(&ledger);
    let records = ledger_lines(&ledger);
    std::fs::remove_dir_all(&dir).expect("temp dir removable");

    let rows: Vec<&str> = csv.lines().filter(|l| !l.starts_with("exp,")).collect();
    assert_eq!(rows.len(), records.len(), "one ledger record per row");
    for terminal in [",ran,", ",dead,", ",halted,", ",abort,"] {
        assert!(
            rows.iter().any(|r| r.contains(terminal)),
            "the golden must cover a {terminal} row"
        );
    }
    assert!(
        rows.iter().any(|r| r.contains(",inf")),
        "the golden must cover a censored quantile"
    );
    assert!(
        records.iter().any(|r| r.contains("spill_fnv")),
        "the golden must cover a spilled run"
    );

    let mut text = String::new();
    for (row, record) in rows.iter().zip(&records) {
        text.push_str(row);
        text.push('\n');
        text.push_str(&format!("  ledger {:016x}\n", fnv(record.as_bytes())));
        if let Some(at) = record.find("\"spill_fnv\":\"") {
            let digest = &record[at + 13..at + 29];
            text.push_str(&format!("  spill {digest}\n"));
        }
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var("SWEEP_GOLDEN_BLESS").as_deref() == Ok("1") {
        std::fs::write(&fixture, &text).expect("fixture writable");
        return;
    }
    let want = std::fs::read_to_string(&fixture).expect("fixture present; bless it once");
    for (i, (got, want)) in text.lines().zip(want.lines()).enumerate() {
        assert_eq!(got, want, "golden line {} differs", i + 1);
    }
    assert_eq!(
        text.lines().count(),
        want.lines().count(),
        "golden length differs"
    );
}
