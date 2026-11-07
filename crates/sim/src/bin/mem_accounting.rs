//! Structural memory accounting over one SMR cell. Usage: `mem_accounting <spec.json> [sample_every] [pool_clients]`

use sim::smr::{Accounting, ClientModel, SmrState, run_smr_grid_observed};
use sim::sweep::{AnySpec, expand_smr, load_specs};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

// Relaxed: single-threaded binary, diagnostic figure only.
static LIVE: AtomicUsize = AtomicUsize::new(0);
// Re-armed once per round, so it reports that round's high-water.
static PEAK: AtomicUsize = AtomicUsize::new(0);
// Never re-armed: the run-wide high-water the memory anchor is quoted against.
static ALLTIME: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Counting {
    fn grew(by: usize) {
        let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
        PEAK.fetch_max(live, Ordering::Relaxed);
        ALLTIME.fetch_max(live, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        Counting::grew(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new >= l.size() {
            Counting::grew(new - l.size());
        } else {
            LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live_bytes() -> u64 {
    LIVE.load(Ordering::Relaxed) as u64
}

fn peak_bytes() -> u64 {
    PEAK.load(Ordering::Relaxed) as u64
}

fn alltime_bytes() -> u64 {
    ALLTIME.load(Ordering::Relaxed) as u64
}

// Called at the END of the observer so the walk's own allocations stay out of the next round.
fn rearm_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn report_peak(peak: &Accounting, live: u64) {
    let total = peak.retained_total();
    println!(
        "peak sample at round {}: retained {:.1} MiB over {} nodes, {} distinct clients",
        peak.round,
        mib(total),
        peak.nodes,
        peak.distinct_clients
    );
    println!(
        "  live heap (allocator truth) {:.1} MiB — the walk is a floor covering {:.1}% of it",
        mib(live),
        100.0 * total as f64 / live as f64
    );
    if !peak.walked_nodes {
        println!(
            "  compact: node, log and checkpoint rows are not walked — observer and tracker \
             rows only, so the walk covers far less of the live heap than for recovery"
        );
        print_rows(peak);
        return;
    }
    println!(
        "  sharing: {} distinct checkpoints, {} distinct logs (of {} nodes) — \
         every checkpoint.* row below is PER FORK CLASS, not per node",
        peak.distinct_checkpoints, peak.distinct_logs, peak.nodes
    );
    let per_holder = peak.rows.get("node.log.entries").copied().unwrap_or(0);
    let deduped = peak
        .rows
        .get("node.log.entries.distinct")
        .copied()
        .unwrap_or(0);
    if deduped > 0 {
        println!(
            "  log chunk sharing: {} distinct sealed chunk allocations across {} distinct logs; \
             {:.2} MiB per-holder against {:.2} MiB deduplicated by chunk address = {:.1}x \
             ACHIEVED. The per-holder row charges a shared chunk once per log that reaches it, \
             which is what keeps it comparable with every pre-chunking figure; this is the \
             measured sharing behind it",
            peak.distinct_log_chunks,
            peak.distinct_logs,
            mib(per_holder),
            mib(deduped),
            per_holder as f64 / deduped as f64
        );
    }
    println!(
        "  state sharing: {} distinct SharedState allocations across node AND checkpoint \
         states (of {} nodes), {} of them reachable from a node — per-holder/per-allocation = \
         {:.2}x duplication over the NODE set alone, so numerator and denominator span one \
         population. The `.distinct` rows below re-count the state bytes once per allocation \
         and are EXCLUDED from the retained total; `checkpoint.state.*.distinct` holds the \
         allocations no node still points at",
        peak.distinct_states,
        peak.nodes,
        peak.distinct_node_states,
        peak.state_duplication()
    );
    let certs = peak
        .rows
        .get("checkpoint.certs.windows")
        .copied()
        .unwrap_or(0);
    let per = peak.distinct_checkpoints * peak.cert_stored_commands;
    if per > 0 {
        println!(
            "  cert windows: {} stored commands per window map (last two per client), \
             {:.0} B per stored command per checkpoint",
            peak.cert_stored_commands,
            certs as f64 / per as f64
        );
    }
    println!(
        "  transient per round (analytic, freed before a walk sees it): {:.1} MiB — models the \
         SHALLOW per-round containers ONLY, never a second log generation",
        mib(peak.transient_per_round)
    );
    let cap = peak.log_capacity_bytes();
    let len = peak.log_len_bytes + peak.perm_len_bytes;
    if len > 0 {
        println!(
            "  log slack: entries {:.2} MiB cap / {:.2} MiB len = {:.3}x, \
             perm {:.2} MiB cap / {:.2} MiB len = {:.3}x, combined {:.3}x \
             (size_of Timed={} Entry={} u32={})",
            mib(peak.rows.get("node.log.entries").copied().unwrap_or(0)),
            mib(peak.log_len_bytes),
            peak.rows.get("node.log.entries").copied().unwrap_or(0) as f64
                / peak.log_len_bytes as f64,
            mib(peak.rows.get("node.log.perm").copied().unwrap_or(0)),
            mib(peak.perm_len_bytes),
            peak.rows.get("node.log.perm").copied().unwrap_or(0) as f64
                / peak.perm_len_bytes as f64,
            cap as f64 / len as f64,
            size_of::<protocol::compact::Timed>(),
            size_of::<protocol::compact::Entry>(),
            size_of::<u32>(),
        );
    }
    print_rows(peak);
}

fn print_rows(peak: &Accounting) {
    let total = peak.retained_total();
    println!("  {:<28} {:>12} {:>8}", "category", "MiB", "share");
    for (name, bytes) in peak.ranked() {
        if bytes == 0 {
            continue;
        }
        println!(
            "  {:<28} {:>12.2} {:>7.1}%",
            name,
            mib(bytes),
            100.0 * bytes as f64 / total as f64
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let spec_path = args
        .get(1)
        .expect("usage: mem_accounting <spec.json> [sample_every] [pool_clients]");
    let every: usize = args.get(2).map_or(20, |s| s.parse().expect("sample_every"));
    let pool: Option<u32> = args.get(3).map(|s| s.parse().expect("pool_clients"));

    let specs = load_specs(spec_path).unwrap_or_else(|e| panic!("{e}"));
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("spec must hold an SMR grid")
    };
    let points = expand_smr(grid).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        points.len(),
        1,
        "isolate a single cell: the spec expanded to {} of them",
        points.len()
    );
    let mut scenario = points[0].1.clone();
    if let Some(clients) = pool {
        scenario.client_model = ClientModel::Pool { clients };
    }
    println!(
        "cell: n={} seed={} rounds={} proto={:?} clients={:?} certs={}",
        scenario.n,
        scenario.seed,
        scenario.max_rounds,
        scenario.proto,
        scenario.client_model,
        if scenario.certs {
            "on"
        } else {
            "OFF — no §5 layer is mounted, so every cert row below is zero by \
             configuration, not by measurement"
        }
    );

    let mut samples: Vec<(Accounting, u64)> = Vec::new();
    let mut live_trace: Vec<u64> = Vec::with_capacity(scenario.max_rounds);
    let mut peak_trace: Vec<u64> = Vec::with_capacity(scenario.max_rounds);
    let live_before_run = live_bytes();
    rearm_peak();
    let report = run_smr_grid_observed(&scenario, &mut |state: &SmrState| {
        // Read the allocator before the walk so its scratch stays out of the figure.
        let live = live_bytes();
        peak_trace.push(peak_bytes());
        live_trace.push(live);
        if state.round().is_multiple_of(every)
            && let Some(acc) = state.memory_accounting()
        {
            samples.push((acc, live));
        }
        rearm_peak();
    });
    assert!(
        !samples.is_empty(),
        "no accounting samples — extended is not accounted"
    );
    println!(
        "terminal={:?} safety_ok={} commands={} pool_peak_in_flight={:?}",
        report.terminal,
        report.safety_ok,
        report.commands.len(),
        report.pool_peak_in_flight
    );

    let peak_at = samples
        .iter()
        .enumerate()
        .max_by_key(|(_, (a, _))| a.retained_total())
        .map(|(i, _)| i)
        .expect("samples");
    let (peak, peak_live) = &samples[peak_at];
    report_peak(peak, *peak_live);

    let max_live = live_trace.iter().copied().max().unwrap_or(0);
    println!(
        "\nlive heap: between-rounds max {:.1} MiB (round {}), all-time high-water {:.1} MiB",
        mib(max_live),
        live_trace.iter().position(|&l| l == max_live).unwrap_or(0) + 1,
        mib(alltime_bytes())
    );
    println!(
        "  the gap between the two is allocated and freed WITHIN one round, and is what a \
         process peak-footprint figure carries that a residency walk cannot"
    );

    let transient = |r: usize| {
        let start = if r == 0 {
            live_before_run
        } else {
            live_trace[r - 1]
        };
        peak_trace[r].saturating_sub(start)
    };
    let (worst, worst_bytes) = (0..peak_trace.len())
        .map(|r| (r, transient(r)))
        .max_by_key(|&(_, b)| b)
        .unwrap_or((0, 0));
    let (peak_round, peak_val) = peak_trace
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|&(_, p)| p)
        .unwrap_or((0, 0));
    println!(
        "\nin-round high-water (PEAK re-armed every round): max {:.1} MiB at round {}",
        mib(peak_val),
        peak_round + 1
    );
    println!(
        "  largest in-round transient {:.1} MiB at round {} (in-round high-water minus the \
         live heap entering that round)",
        mib(worst_bytes),
        worst + 1
    );
    if peak.walked_nodes {
        println!(
            "  predicted by the two-generation overlap: node.log.entries + node.log.perm at the \
             peak sample = {:.1} MiB",
            mib(peak.log_capacity_bytes())
        );
    }

    let (interned, attempted) = protocol::recovery::state_intern_counts();
    if attempted > 0 {
        println!(
            "\nboundary state interning {interned} of {attempted} shared mints ({:.2}%) — how \
             often a class member's own state compared equal to the class allocation and could \
             hold it instead. A representation that stops comparing equal shows up HERE and \
             nowhere else: the byte gates stay green and the memory rows merely go flat",
            100.0 * interned as f64 / attempted as f64
        );
    }

    let (clones, entries, sn_entries, handles, sites) = protocol::shared_state::state_clone_stats();
    let entry_bytes = size_of::<protocol::compact::Entry>() as u64;
    let tail_entries = entries.saturating_sub(handles * protocol::shared_state::K_STATE as u64);
    println!(
        "\ndeep state copies {clones} ({:.3} per node-round), copying {entries} entries \
         ({:.1} MiB flat) and {sn_entries} sn lane entries; as chunked {handles} sealed handles \
         + {tail_entries} tail entries = {:.1} MiB (K_STATE={})",
        clones as f64 / (live_trace.len().max(1) as f64 * scenario.n as f64),
        mib(entries * entry_bytes),
        mib(handles * 8 + tail_entries * entry_bytes),
        protocol::shared_state::K_STATE,
    );
    println!(
        "  by site: compact_cow_append={} compact_cow_forget={} recovery_cow_execute={} \
         recovery_cow_forget={} (unattributed={})",
        sites[protocol::shared_state::CLONE_SITE_COMPACT_COW_APPEND],
        sites[protocol::shared_state::CLONE_SITE_COMPACT_COW_FORGET],
        sites[protocol::shared_state::CLONE_SITE_RECOVERY_COW_EXECUTE],
        sites[protocol::shared_state::CLONE_SITE_RECOVERY_COW_FORGET],
        clones.saturating_sub(sites.iter().sum::<u64>())
    );

    let (seed, gather, probes, folds) = protocol::merge::merge_touch_counts();
    let touches = seed + gather + probes + folds;
    if touches > 0 {
        let pct = |part: u64| 100.0 * part as f64 / touches as f64;
        let (merges, chosen, cp, alloc) = protocol::merge::merge_shape_counts();
        println!(
            "\nmerge entry-touches {touches}: seed {seed} ({:.1}%) gather {gather} ({:.1}%) \
             probes {probes} ({:.1}%) folds {folds} ({:.1}%) — the seed and the gather are the \
             removable pair, {:.1}% together",
            pct(seed),
            pct(gather),
            pct(probes),
            pct(folds),
            pct(seed + gather)
        );
        println!(
            "  shape: {merges} copying merges, {chosen} chosen-log entries of which {cp} \
             ({:.1}%) were already agreed, {alloc} merged-log entries allocated — the median \
             seed is {:.1}% of them",
            100.0 * cp as f64 / chosen.max(1) as f64,
            100.0 * seed as f64 / alloc.max(1) as f64
        );
    }

    let skipped = protocol::merge::union_skipped_entries();
    if skipped > 0 {
        println!(
            "\nunion chunk skip {skipped} entries not probed because their chunk was the \
             median's own; {probes} probed, so the walk 0b measured was {} and the skip removed \
             {:.1}% of it",
            probes + skipped,
            100.0 * skipped as f64 / (probes + skipped) as f64
        );
    }

    let (windows, points, node_visits) = sim::smr::census_counts();
    if windows > 0 {
        println!(
            "\nposition census {points} points computed over {windows} open windows, \
             visiting {node_visits} nodes — under a scoped-out census the windows are \
             unchanged and the other two are 0, which is what separates skipping the \
             COMPUTATION from moving the window predicate"
        );
    } else {
        println!(
            "\nposition census NO WINDOW OPENED — not a scoped-out census but a run \
             that tracked nothing; the counters cannot distinguish those, so this \
             says which one it saw"
        );
    }

    println!(
        "\nholder Vec allocations {} — one per distinct entry per round under the \
         positional index, 0 under the count-only build",
        sim::smr::holder_vec_count()
    );

    let (probes, pre_calls, pre_entries, recheck_calls, recheck_entries) =
        sim::smr::boundary_key_counts();
    println!(
        "\nboundary class-keying {probes} map probes over {pre_calls} would_carry_pre calls, \
         materialising {pre_entries} entries ({:.1} per call); {recheck_calls} of those calls \
         ({:.1}%) re-derived an already-keyed REPRESENTATIVE's prefix, materialising \
         {recheck_entries} entries ({:.1}% of the total) — a class of size m re-derives its \
         representative m−1 times, and that subset is what a keying lever removes",
        pre_entries as f64 / pre_calls.max(1) as f64,
        100.0 * recheck_calls as f64 / pre_calls.max(1) as f64,
        100.0 * recheck_entries as f64 / pre_entries.max(1) as f64,
    );

    let (noop, unions) = protocol::merge::noop_union_counts();
    if unions > 0 {
        println!(
            "\nfast-path hits {noop} of {unions} merges tested ({:.2}%) — how often every chosen \
             log was element-identical to the median with nothing appended, so the median's \
             handles could be adopted instead of copied",
            100.0 * noop as f64 / unions as f64
        );
    }

    println!(
        "\ntrajectory (round, live MiB, retained MiB, in-round peak MiB, transient MiB, \
         cert-window MiB per class, clients, stored):"
    );
    for (s, live) in &samples {
        let r = s.round - 1;
        println!(
            "  {:>5} {:>10.1} {:>10.1} {:>10.1} {:>10.1} {:>8.1} {:>6} {:>6}",
            s.round,
            mib(*live),
            mib(s.retained_total()),
            mib(peak_trace[r]),
            mib(transient(r)),
            mib(s.rows.get("checkpoint.certs.windows").copied().unwrap_or(0)),
            s.distinct_clients,
            s.cert_stored_commands
        );
    }

    println!("\nobserver trajectory (round, canonical KiB, indexes KiB, exec_dup KiB):");
    let kib = |s: &Accounting, key| s.rows.get(key).copied().unwrap_or(0) as f64 / 1024.0;
    for (s, _) in &samples {
        println!(
            "  {:>5} {:>10.1} {:>10.1} {:>10.1}",
            s.round,
            kib(s, "observer.canonical"),
            kib(s, "observer.indexes"),
            kib(s, "observer.exec_dup")
        );
    }
}
