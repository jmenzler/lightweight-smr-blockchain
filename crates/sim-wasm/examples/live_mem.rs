//! Live-session heap per command on the lab default scenario. Usage: `live_mem [rounds]`

use serde_json::Value;
use sim_wasm::LiveSmr;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        grew(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new >= l.size() {
            grew(new - l.size());
        } else {
            LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn spec() -> String {
    let w: Vec<f64> = (0..9).map(|i| 1.0 / (i as f64 + 1.0)).collect();
    let sum: f64 = w.iter().sum();
    let pmf: Vec<f64> = w.iter().map(|v| v / sum).collect();
    format!(
        r#"{{"n":512,"seed":1,"k":6,"ell":3,"sigma":1.0,
            "proto":{{"kind":"recovery","t_window_rounds":48,"resend_until_acked":true}},
            "injections":[{{"round":2,"client":1,"op":1}}],"max_rounds":5000,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}},
            "traffic":{{"kind":"pmf","arrivals_pmf":{}}}}}"#,
        serde_json::to_string(&pmf).unwrap()
    )
}

fn kib(b: f64) -> f64 {
    b / 1024.0
}

fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .map_or(500, |a| a.parse().expect("rounds"));
    assert!(
        rounds >= 100,
        "a slope needs two samples taken 50 rounds apart: run at least 100 rounds"
    );
    let spec = spec();
    let t: u64 = serde_json::from_str::<Value>(&spec).unwrap()["proto"]["t_window_rounds"]
        .as_u64()
        .expect("a recovery spec");
    let base = LIVE.load(Ordering::Relaxed);
    let mut live = LiveSmr::new(&spec).expect("spec");
    drop(live.report_delta_json());
    let mut delta_bytes = 0usize;
    let mut marks: Vec<(usize, usize, usize)> = Vec::new();
    for r in 1..=rounds {
        PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        drop(live.step(0.1));
        let delta = live.report_delta_json();
        delta_bytes += delta.len();
        drop(delta);
        let peak = PEAK.load(Ordering::Relaxed) - base;
        if r % 50 == 0 {
            let report: Value = serde_json::from_str(&live.report_json()).unwrap();
            let cmds = report["commands"].as_array().unwrap().len();
            drop(report);
            let heap = LIVE.load(Ordering::Relaxed) - base;
            marks.push((r, cmds, heap));
            println!(
                "round {r:5}  commands {cmds:5}  heap {:8.1} KiB  {:6.2} KiB/cmd  step peak {:8.1} KiB",
                kib(heap as f64),
                kib(heap as f64) / cmds.max(1) as f64,
                kib(peak as f64)
            );
        }
    }
    let (r0, c0, h0) = marks[(marks.len() - 1) / 2];
    let (r1, c1, h1) = *marks.last().unwrap();
    let slope_cmd = (h1 as f64 - h0 as f64) / (c1 - c0) as f64;
    let slope_round = (h1 as f64 - h0 as f64) / (r1 - r0) as f64;
    println!(
        "slope over rounds {r0}..{r1}: {:.2} KiB/cmd, {:.2} KiB/round; deltas sent {:.1} KiB total",
        kib(slope_cmd),
        kib(slope_round),
        kib(delta_bytes as f64)
    );

    let report: Value = serde_json::from_str(&live.report_json()).unwrap();
    let cmds = report["commands"].as_array().unwrap();
    let point = std::mem::size_of::<sim::smr::SpreadPoint>();
    let attempt = std::mem::size_of::<sim::smr::DeliveryAttempt>();
    let command = std::mem::size_of::<sim::smr::CommandReport>();
    let (mut points, mut needed, mut attempts, mut amp) = (0usize, 0usize, 0usize, 0usize);
    let (mut ignored, mut spread_cap, mut attempts_cap) = (0usize, 0usize, 0usize);
    let vec_cap = |len: usize| {
        if len == 0 {
            0
        } else {
            len.next_power_of_two().max(4)
        }
    };
    for c in cmds {
        let spread = c["spread"].as_array().unwrap();
        points += spread.len();
        let cut = [
            c["prefix_fixed_round"].as_u64(),
            c["all_logs_round"].as_u64().map(|r| r + 1),
        ]
        .into_iter()
        .flatten()
        .max();
        needed += match cut {
            Some(cut) if c["executed_round"].as_u64().is_some() => spread
                .iter()
                .filter(|p| p["round"].as_u64().unwrap() <= cut)
                .count(),
            _ => spread.len(),
        };
        let a = c["attempts"].as_array().unwrap();
        attempts += a.len();
        ignored += a.iter().filter(|x| x["outcome"] == "Ignored").count();
        amp += c["amp_receivers"].as_array().unwrap().len();
        spread_cap += vec_cap(spread.len()) * point;
        attempts_cap += vec_cap(a.len()) * attempt;
    }
    cut_check(&report, t);
    let n = cmds.len() as f64;
    let json = live.report_json().len() as f64;
    println!(
        "tracker lists by Vec capacity: spread {:.2} KiB/cmd, attempts {:.2} KiB/cmd; report JSON {:.2} KiB/cmd",
        kib(spread_cap as f64 / n),
        kib(attempts_cap as f64 / n),
        kib(json / n)
    );
    println!(
        "{} commands; per command: {:.1} spread points ({} B each, {:.2} KiB), {:.1} needed past execution; \
         {:.1} attempts ({} B each, {:.2} KiB, {:.1} Ignored); {:.1} amp receivers; CommandReport {} B",
        cmds.len(),
        points as f64 / n,
        point,
        kib((points * point) as f64 / n),
        needed as f64 / n,
        attempts as f64 / n,
        attempt,
        kib((attempts * attempt) as f64 / n),
        ignored as f64 / n,
        amp as f64 / n,
        command,
    );
}

/// What a per-command cut at max(T_E, T_B + 1) would remove that the lab reads.
fn cut_check(report: &Value, t: u64) {
    let cmds = report["commands"].as_array().unwrap();
    let rounds = report["metrics"].as_array().unwrap().len() as u64;
    let boundary = (rounds - 1) / t * t;
    let (mut executed, mut te_later, mut moved, mut open_past) = (0, 0, 0, 0);
    let (mut te_tb, mut ex_te) = (Vec::new(), Vec::new());
    for c in cmds {
        let (Some(te), Some(tb)) = (
            c["prefix_fixed_round"].as_u64(),
            c["all_logs_round"].as_u64(),
        ) else {
            continue;
        };
        let Some(ex) = c["executed_round"].as_u64() else {
            open_past += usize::from(te < boundary);
            continue;
        };
        executed += 1;
        te_later += usize::from(te > tb + 1);
        te_tb.push(te as i64 - tb as i64);
        ex_te.push(ex as i64 - te as i64);
        let mut tail = c["spread"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["round"].as_u64().unwrap() >= te)
            .map(|p| (&p["pos_min"], &p["pos_med"], &p["pos_max"]));
        let first = tail.next();
        moved += usize::from(tail.any(|p| Some(p) != first));
    }
    if executed == 0 {
        println!("no executed command with T_E yet: run more rounds for the cut probe");
        return;
    }
    te_tb.sort();
    ex_te.sort();
    println!(
        "executed {executed}: T_E > T_B + 1 in {te_later}; median T_E - T_B {}, median executed - T_E {}; \
         position moved after T_E in {moved}; open commands with T_E before boundary r{boundary}: {open_past}",
        te_tb[te_tb.len() / 2],
        ex_te[ex_te.len() / 2],
    );
}
