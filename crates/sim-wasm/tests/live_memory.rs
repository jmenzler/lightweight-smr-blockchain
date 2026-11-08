//! Heap pins for a live session, read off a counting allocator: one test, so no other test's
//! allocations land in the count.

use sim::smr::{CommandReport, DeliveryAttempt, SpreadPoint};
use sim_wasm::LiveSmr;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new >= l.size() {
            LIVE.fetch_add(new - l.size(), Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const SPEC: &str = r#"{"n":64,"seed":1,"k":6,"ell":3,"sigma":1.5,
    "proto":{"kind":"recovery","t_window_rounds":24,"resend_until_acked":true},
    "injections":[{"round":2,"client":1,"op":1}],"max_rounds":400,
    "schedule":{"kind":"fresh_per_round","fraction":0.1},
    "traffic":{"kind":"pmf","arrivals_pmf":[0.2,0.3,0.3,0.2]}}"#;
const ROUNDS: usize = 300;

/// Heap a session holds after `ROUNDS` steps, and its command count.
fn session_heap(with_deltas: bool) -> (usize, usize) {
    let before = LIVE.load(Ordering::Relaxed);
    let mut live = LiveSmr::new(SPEC).unwrap();
    for _ in 0..ROUNDS {
        drop(live.step(0.1));
        if with_deltas {
            drop(live.report_delta_json());
        }
    }
    assert!(
        !live.step(0.1).contains(r#""absorbed":true"#),
        "the session ran to the end"
    );
    let held = LIVE
        .load(Ordering::Relaxed)
        .checked_sub(before)
        .expect("a session holds heap");
    let report: serde_json::Value = serde_json::from_str(&live.report_json()).unwrap();
    (held, report["commands"].as_array().unwrap().len())
}

/// Bytes per command a full copy would spend on the two lists alone.
fn copied_list_bytes() -> usize {
    let mut live = LiveSmr::new(SPEC).unwrap();
    for _ in 0..ROUNDS {
        live.step(0.1);
    }
    let report: serde_json::Value = serde_json::from_str(&live.report_json()).unwrap();
    let commands = report["commands"].as_array().unwrap();
    let total: usize = commands
        .iter()
        .map(|c| {
            c["spread"].as_array().unwrap().len() * size_of::<SpreadPoint>()
                + c["attempts"].as_array().unwrap().len() * size_of::<DeliveryAttempt>()
        })
        .sum();
    total / commands.len()
}

#[test]
fn the_delta_cursor_holds_far_less_than_a_copy_of_the_report() {
    let (plain, commands) = session_heap(false);
    let (tracked, again) = session_heap(true);
    assert_eq!(commands, again, "the two sessions are the same run");
    assert!(
        commands > 400,
        "enough commands to read a slope: {commands}"
    );
    let per_command = tracked
        .checked_sub(plain)
        .expect("the cursor adds heap, never frees the session's")
        / commands;
    // scalars, two marks and hash-map slack; nothing that grows with the lists
    let bound = 4 * size_of::<CommandReport>();
    let copy = copied_list_bytes();
    println!("delta cursor {per_command} B/cmd; bound {bound}; a copy's lists alone {copy}");
    assert!(
        copy > bound,
        "the scenario's lists must outweigh the bound to discriminate"
    );
    assert!(
        per_command <= bound,
        "the delta cursor holds {per_command} B per command (bound {bound})"
    );
}
