// Run from the repository root: node tools/lab/tests/smr_dashboard.mjs
// The dashboard's derived series: backlog stages, safety chip states, the servers axis and
// the commit margin (recovery adds its slack to checkpoint). Unit cases first, then engine runs through the checked-in
// sim_wasm build: the stage stack must equal the backlog every round, and the newest P's
// commands (not executed when the run ends) must count as frozen.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import {
  derive, stageKeys, stageBounds, stageSeries, frozenAt, lastFrozenP, safetyChecks, serverAxis,
  slackToCheckpoint, lifelines, shownMargin, blockMinMargin, cmdHighlight, nextSelection, blockLife,
  checksLabel, displacements, withinShare, committedOrder, fifoCheck, inViewSet, spreadCurve, stageLatencies, sideTabAfter, formatBytes, memoryLevel,
  committedEntries, rollbackRounds, staleNote, entryMark, listWindow, slowAbove, verifyIndex, certRecord, snIndex, syncedEntry, pickedEntry,
} from '../smr-derive.js';

const cmd = (extra = {}) => ({
  client: 1, op: 1, auto: false, injection_round: 2, delivered_round: 3, all_logs_round: 6,
  prefix_fixed_round: 8, executed_round: null, committed_round: null, attempts: [], spread: [], ...extra,
});

/* ---------- selection: block tint, one command white ---------- */

{
  const block = new Set([3, 4, 5]);
  assert.equal(cmdHighlight(4, null, block), 'block', 'a selected block tints its commands');
  assert.equal(cmdHighlight(9, null, block), 'none');
  assert.equal(cmdHighlight(4, 4, block), 'command', 'the selected command outranks its block');
  assert.equal(cmdHighlight(5, 4, block), 'block', 'the rest of its block stays tinted');
  assert.equal(cmdHighlight(9, 9, null), 'command', 'a command picked without a block');
  assert.equal(cmdHighlight(9, null, null), 'none');

  let s = { blockSel: null, sel: null };
  s = nextSelection(s, { block: 5 });
  assert.deepEqual(s, { blockSel: 5, sel: null });
  s = nextSelection(s, { cmd: 4 });
  assert.deepEqual(s, { blockSel: 5, sel: 4 });
  s = nextSelection(s, { cmd: 4 });
  assert.deepEqual(s, { blockSel: 5, sel: null }, 'deselecting the command returns to the block tint');
  s = nextSelection(nextSelection(s, { cmd: 7 }), { block: 5 });
  assert.deepEqual(s, { blockSel: null, sel: null }, 'deselecting the block clears both');
  s = nextSelection(nextSelection({ blockSel: 5, sel: 4 }, { block: 6 }), { cmd: 2 });
  assert.deepEqual(s, { blockSel: 6, sel: 2 }, 'another block drops the old command');
  assert.deepEqual(nextSelection({ blockSel: 5, sel: 4 }, { block: null }), { blockSel: null, sel: null }, 'closing the block drawer');
}

/* ---------- sidebar tab: Summary until a command is selected; a pick holds until the next selection ---------- */

{
  let t = { tab: 'summary', explicit: false };
  t = sideTabAfter(t, { select: 4 });
  assert.deepEqual(t, { tab: 'audit', explicit: false }, 'selecting a command shows its audit trail');
  t = sideTabAfter(t, { select: null });
  assert.deepEqual(t, { tab: 'summary', explicit: false }, 'nothing selected: back to the summary');
  t = sideTabAfter(t, { pick: 'audit' });
  assert.deepEqual(t, { tab: 'audit', explicit: true });
  assert.deepEqual(sideTabAfter(t, { select: null }), t, 'a manual pick outlasts a deselection');
  t = sideTabAfter(t, { pick: 'summary' });
  assert.deepEqual(sideTabAfter(t, { select: 7 }), { tab: 'audit', explicit: false }, 'every new selection shows its audit trail again');
}

/* ---------- memory: byte formatting and the page-limit warning ---------- */

{
  assert.equal(formatBytes(512), '512 B');
  assert.equal(formatBytes(120664), '118 KiB');
  assert.equal(formatBytes(1.5 * 1024 * 1024), '1.5 MiB');
  assert.equal(formatBytes(3 * 1024 ** 3), '3.0 GiB');
  assert.equal(formatBytes(null), '–');
  // a wasm32 page holds at most 4 GiB of linear memory; browsers often stop earlier
  assert.equal(memoryLevel(200 * 1024 ** 2), 'ok');
  assert.equal(memoryLevel(1.5 * 1024 ** 3), 'warn');
  assert.equal(memoryLevel(3 * 1024 ** 3), 'high');
  assert.equal(memoryLevel(null), 'ok');
}

/* ---------- a block's life: first delivery → spread → fixed → frozen → executed ---------- */

{
  const cmds = [
    cmd({ delivered_round: 100, all_logs_round: 106, prefix_fixed_round: 120, executed_round: 240 }),
    cmd({ delivered_round: 110, all_logs_round: 115, prefix_fixed_round: 125, executed_round: 240 }),
    cmd({ delivered_round: 130, all_logs_round: 137, prefix_fixed_round: 150, executed_round: 240 }),
    cmd({ delivered_round: 5, executed_round: 96 }),
  ];
  const block = { round: 240, cmds: [0, 1, 2] };
  assert.deepEqual(blockLife({ commands: cmds }, block, 'recovery', 48),
    { from: 100, spread: [110, 115], fixed: 125, frozen: 192, end: 240 });
  // compact: no freeze; the tile is the commit window
  assert.deepEqual(blockLife({ commands: cmds }, { round: 240, cmds: [0, 1] }, 'compact', 48),
    { from: 100, spread: [110, 115], fixed: 125, frozen: null, end: 240 }, 'median of an even count is the upper one');
  // no position fixed recorded (batch): no tick
  const batch = cmds.map(c => ({ ...c, prefix_fixed_round: null }));
  assert.equal(blockLife({ commands: batch }, block, 'recovery', 48).fixed, null);
  assert.equal(blockLife({ commands: cmds }, { round: 48, cmds: [] }, 'recovery', 48), null, 'an empty block has no life');
}

/* ---------- stage keys and bounds ---------- */

assert.deepEqual(stageKeys('recovery', true), ['client', 'ordering', 'waiting', 'frozen']);
assert.deepEqual(stageKeys('recovery', false), ['client', 'waiting', 'frozen'], 'no position-fixed landmark: ordering and waiting merge');
assert.deepEqual(stageKeys('compact', true), ['client', 'ordering', 'waiting']);
assert.deepEqual(stageKeys('compact', false), ['client', 'waiting']);
assert.deepEqual(stageKeys('extended', true), ['client', 'spreading']);
assert.deepEqual(stageKeys('extended', false), ['client', 'spreading']);

{
  // recovery: freeze = executed − T, half-open intervals ending at the commit
  const b = stageBounds(cmd({ executed_round: 96 }), 'recovery', 48, true, null);
  assert.deepEqual(b.bounds, [2, 3, 8, 48, 96]);
  assert.equal(b.clamped, false);
  // not executed and not in the newest P: open stages run to the run end (Infinity)
  assert.deepEqual(stageBounds(cmd(), 'recovery', 48, true, null).bounds, [2, 3, 8, Infinity, Infinity]);
  // a command in the newest P counts as frozen from that boundary
  assert.deepEqual(stageBounds(cmd(), 'recovery', 48, true, 48).bounds, [2, 3, 8, 48, Infinity]);
  // merged: no fixed bound
  assert.deepEqual(stageBounds(cmd({ executed_round: 96 }), 'recovery', 48, false, null).bounds, [2, 3, 48, 96]);
}

{
  // a position fixed only after the freeze: the command goes straight from ordering to frozen
  const b = stageBounds(cmd({ prefix_fixed_round: 60, executed_round: 96 }), 'recovery', 48, true, null);
  assert.deepEqual(b.bounds, [2, 3, 48, 48, 96]);
  assert.equal(b.clamped, true);
  // never delivered yet executed cannot occur, but a missing bound never exceeds the next one
  assert.deepEqual(stageBounds(cmd({ delivered_round: null, prefix_fixed_round: null }), 'recovery', 48, true, null).bounds,
    [2, Infinity, Infinity, Infinity, Infinity]);
}

{
  // compact waits for its commit round; extended's backlog ends in every log
  assert.deepEqual(stageBounds(cmd({ committed_round: 40 }), 'compact', 24, true, null).bounds, [2, 3, 8, 40]);
  assert.deepEqual(stageBounds(cmd({ committed_round: 40 }), 'compact', 24, false, null).bounds, [2, 3, 40]);
  assert.deepEqual(stageBounds(cmd(), 'extended', null, true, null).bounds, [2, 3, 6]);
}

{
  // the series: per round counts, summing to the backlog
  const rep = {
    metrics: Array.from({ length: 10 }, () => ({ max_executed_len: 0 })),
    commands: [
      cmd({ injection_round: 1, delivered_round: 2, prefix_fixed_round: 3, committed_round: 6 }),
      cmd({ injection_round: 2, delivered_round: 4, prefix_fixed_round: 9, committed_round: null }),
      cmd({ injection_round: 12, delivered_round: null, prefix_fixed_round: null }),
    ],
  };
  const s = stageSeries(rep, 'compact', 4, true);
  assert.deepEqual(s.keys, ['client', 'ordering', 'waiting']);
  assert.deepEqual([...s.counts.client.slice(1)], [1, 1, 1, 0, 0, 0, 0, 0, 0, 0]);
  assert.deepEqual([...s.counts.ordering.slice(1)], [0, 1, 0, 1, 1, 1, 1, 1, 0, 0]);
  assert.deepEqual([...s.counts.waiting.slice(1)], [0, 0, 1, 1, 1, 0, 0, 0, 1, 1]);
  const pending = derive(rep, 'compact').pending;
  for (let r = 1; r <= 10; r++) assert.equal(s.counts.client[r] + s.counts.ordering[r] + s.counts.waiting[r], pending[r]);
  assert.equal(stageSeries(rep, 'compact', 4, true), s, 'cached on identity');
}

/* ---------- frozen membership from positions at the boundary ---------- */

{
  // P(B) covers the pre-pass positions [executed before the pass + |P(B − T)|, … + |P(B)|)
  // (the window-end row is read before the pass; |P(B)| lands on the row after B)
  const T = 4;
  const metrics = Array.from({ length: 10 }, (_, i) => ({ max_executed_len: i + 1 > 8 ? 2 : 0 }));
  const rows = Array.from({ length: 10 }, () => ({ max_cp_p_len: 0, cp_fork_k: 1 }));
  rows[4].max_cp_p_len = 2;
  rows[8].max_cp_p_len = 2;
  const at = (round, pos) => ({ round, useful_holders: 1, useful_total: 1, pos_min: pos, pos_med: pos, pos_max: pos });
  const rep = {
    metrics, recovery: { rounds: rows },
    commands: [
      cmd({ executed_round: 8, spread: [at(4, 0), at(8, 0)] }),
      cmd({ executed_round: 8, spread: [at(4, 1), at(8, 1)] }),
      cmd({ spread: [at(4, 2), at(8, 4)] }),
      cmd({ spread: [at(8, 5)] }),
      cmd({ spread: [at(8, 6)] }),
      cmd({ spread: [{ round: 8, useful_holders: 0, useful_total: 1 }] }),
    ],
  };
  assert.deepEqual([...frozenAt(rep, 4, T)], [0, 1], 'P(4) = [0, 2)');
  assert.deepEqual([...frozenAt(rep, 8, T)], [], 'P(8) = [0 + |P(4)|, 4) = [2, 4): positions 4.. lie past it');
  rows[8].max_cp_p_len = 4;
  assert.deepEqual([...frozenAt(rep, 8, T)], [2, 3], 'P(8) = [2, 6); positions 2 and 3 are entries that are not commands');
  const last = lastFrozenP(rep, T);
  assert.equal(last.boundary, 8);
  assert.deepEqual([...last.members], [2, 3]);
  rows[8].cp_fork_k = 2;
  assert.deepEqual(lastFrozenP(rep, T), { boundary: 8, members: null, forkK: 2 }, 'two checkpoints at the newest window: membership unknown, and said so');
  rows[8].cp_fork_k = 1;
  const ended = { ...rep, metrics: metrics.slice(0, 8), recovery: { rounds: rows.slice(0, 8) } };
  assert.equal(lastFrozenP(ended, T)?.boundary ?? null, 4, 'a run that ends on a boundary has no row with its |P| yet: the one before');
}

/* ---------- safety chips ---------- */

{
  const base = {
    metrics: [{ max_executed_len: 0 }, { max_executed_len: 0 }], commands: [], safety_ok: true,
    recovery: { fork_ok: true, rounds: [{ cp_fork_k: 1 }, { cp_fork_k: 1 }] },
  };
  const chips = safetyChecks(base, 'recovery', null);
  assert.deepEqual(chips.map(c => c.key), ['fork', 'monotone', 'useful', 'boundary', 'lineage', 'committedCp', 'once']);
  assert.deepEqual(chips.map(c => c.state), ['vacuous', 'vacuous', 'vacuous', 'vacuous', 'ok', 'ok', 'vacuous'],
    'nothing executed: the executed-sequence checks cannot fail yet');
  assert.deepEqual(safetyChecks(base, 'compact', null).map(c => c.key), ['fork', 'monotone', 'useful', 'once']);
  assert.deepEqual(safetyChecks(base, 'extended', null), [], 'extended has no commit and no checker');

  const ran = { ...base, metrics: [{ max_executed_len: 0 }, { max_executed_len: 3 }] };
  assert.ok(safetyChecks(ran, 'recovery', null).every(c => c.state === 'ok'), 'absent fields mean 0 / none');

  const bad = {
    ...ran, safety_ok: false, retractions: 2, first_retraction_round: 7, strong_safety_violation_rounds: 3,
    strong_safety_first_violation: 5, strong_safety_boundary_violations: 1, strong_safety_boundary_first_violation: 48,
    exec_dup_round: 9,
    recovery: { fork_ok: false, commit_fork_round: 96, rounds: [{ cp_fork_k: 1 }, { cp_fork_k: 3 }] },
  };
  const got = Object.fromEntries(safetyChecks(bad, 'recovery', 11).map(c => [c.key, [c.state, c.at]]));
  assert.deepEqual(got, {
    fork: ['fail', 11], monotone: ['fail', 7], useful: ['fail', 5], boundary: ['fail', 48],
    lineage: ['fail', 2], committedCp: ['fail', 96], once: ['fail', 9],
  });
  // a failed fork without an observed round (a report loaded whole) names none
  assert.equal(safetyChecks(bad, 'recovery', null)[0].at, null);
  // the cliff shape: lineage fails before anything executes
  const cliff = { ...base, recovery: { fork_ok: false, commit_fork_round: 6, rounds: [{ cp_fork_k: 1 }, { cp_fork_k: 11 }] } };
  assert.deepEqual(safetyChecks(cliff, 'recovery', null).map(c => c.state),
    ['vacuous', 'vacuous', 'vacuous', 'vacuous', 'fail', 'fail', 'vacuous']);
  // one line of evidence per failed check
  const cliffRows = { ...base, recovery: { fork_ok: false, commit_fork_round: 6, rounds: [{ cp_fork_k: 1, window: 2 }, { cp_fork_k: 11, window: 2 }] } };
  const ev = Object.fromEntries(safetyChecks(cliffRows, 'recovery', null).filter(c => c.state === 'fail').map(c => [c.key, c.evidence]));
  assert.deepEqual(ev, {
    lineage: '11 distinct checkpoints in window 2 (r2)',
    committedCp: 'two distinct committed checkpoints at window end r6',
  });
  assert.equal(Object.fromEntries(safetyChecks(bad, 'recovery', 11).map(c => [c.key, c.evidence])).monotone, '2 retractions');
  // the indicator: never "0 of 7 hold"
  assert.equal(checksLabel(safetyChecks(ran, 'recovery', null)), '✓ 7 passed');
  assert.equal(checksLabel(safetyChecks(cliff, 'recovery', null)), '✕ 2 failed · 5 not applicable');
  assert.equal(checksLabel(safetyChecks(base, 'recovery', null)), '✓ 2 passed · 5 not applicable');
  assert.equal(checksLabel(safetyChecks(base, 'compact', null)), '○ 4 not applicable', 'nothing passed yet: neutral, no check mark');

  // a failure is never masked as not applicable
  assert.equal(safetyChecks({ ...base, exec_dup_round: 9 }, 'recovery', null).find(c => c.key === 'once').state, 'fail');
  assert.equal(safetyChecks({ ...base, safety_ok: false }, 'compact', null)[0].state, 'fail');
  // the window-end row is read before its pass: commands executed at it count as executed
  const atBoundary = { ...base, commands: [cmd({ executed_round: 2 })] };
  assert.ok(safetyChecks(atBoundary, 'recovery', null).every(c => c.state === 'ok'), 'executed at the last window end');
  // a report without the fields (an older format) records nothing: never shown as passing
  const old = { metrics: base.metrics, commands: [] };
  assert.ok(safetyChecks(old, 'compact', null).every(c => c.state === 'unrecorded'));
  assert.equal(checksLabel(safetyChecks(old, 'compact', null)), '? 4 not recorded');
  const noRec = { ...ran, recovery: undefined };
  assert.deepEqual(safetyChecks(noRec, 'recovery', null).filter(c => c.state === 'unrecorded').map(c => c.key), ['lineage', 'committedCp']);
}

/* ---------- ordering: tie-aware displacement, per-client FIFO, in-view scope ---------- */

{
  const at = rounds => ({ commands: rounds.map((r, i) => cmd({ client: i + 1, injection_round: r })) });
  // submission ranks by injection round; commands injected in one round are tied
  const rep = at([5, 5, 6, 7]);
  assert.deepEqual(displacements(rep, [2, 0, 1, 3]), [-2, 0, 1, 0], 'a command anywhere inside its round\'s group counts 0');
  assert.deepEqual(displacements(rep, [0, 1, 2, 3]), [0, 0, 0, 0]);
  assert.deepEqual(displacements(rep, [1, 0, 2, 3]), [0, 0, 0, 0], 'swapping a tied pair is no displacement');
  assert.deepEqual(displacements(rep, [3, 2]), [-1, 1], 'ranks are among the given commands only');
  assert.equal(withinShare([0, 1, -3, 4, -9], 3), 0.6);
  assert.equal(withinShare([], 3), null);

  // the committed sequence as report indices; entries that are not known commands are skipped
  const cmds = [cmd({ client: 1 }), cmd({ client: 2 }), cmd({ client: 1 })];
  const e = (client, sn) => ({ kind: 'cmd', client, sn });
  assert.deepEqual(committedOrder({ commands: cmds }, [e(2, 1), { kind: 'nop', op: 3 }, e(1, 2), e(9, 1), e(1, 1)]), [1, 2, 0]);
  // positions are chain positions: a no-op or ⊥ entry still takes one
  assert.deepEqual(committedEntries({ commands: cmds }, [e(2, 1), { kind: 'nop', op: 3 }, e(1, 2), { kind: 'null', client: 4, sn: 1 }, e(1, 1)], 198),
    [{ cmd: 1, pos: 199 }, { cmd: 2, pos: 201 }, { cmd: 0, pos: 203 }]);

  // per client, issue order (report order) must be commit order
  assert.deepEqual(fifoCheck({ commands: cmds }, [0, 2, 1]), { clients: 2, multi: 1, pairs: 1, violations: 0, status: 'ok' });
  // a scope restricts which pairs are reported, never what counts as committed
  assert.deepEqual(fifoCheck({ commands: cmds }, [0, 1, 2], new Set([2])), { clients: 2, multi: 1, pairs: 1, violations: 0, status: 'ok' },
    'client 1\'s earlier command committed outside the scope (another block, before the view)');
  assert.equal(fifoCheck({ commands: cmds }, [2, 1, 0], new Set([2])).violations, 1);
  assert.equal(fifoCheck({ commands: cmds }, [0, 1, 2], new Set([1])).pairs, 0, 'no scoped command has an earlier one');
  assert.equal(fifoCheck({ commands: cmds }, [2, 0, 1]).status, 'fail');
  assert.equal(fifoCheck({ commands: cmds }, [2, 1]).violations, 1, 'a later command committed before an earlier one');
  assert.deepEqual(fifoCheck({ commands: cmds }, [1]), { clients: 2, multi: 1, pairs: 0, violations: 0, status: 'na' }, 'no pair to compare: not applicable');
  assert.equal(fifoCheck({ commands: cmds }, [0, 1, 2], new Set([0])).status, 'na', 'a scope holding only a client\'s first command');
  assert.equal(fifoCheck({ commands: [cmd({ client: 1 }), cmd({ client: 2 })] }, [0, 1]).status, 'na', 'no client issued two commands');

  // in view = delivered inside the visible rounds
  const view = { commands: [cmd({ delivered_round: 10 }), cmd({ delivered_round: 50 }), cmd({ delivered_round: null }), cmd({ delivered_round: 60 })] };
  assert.deepEqual([...inViewSet(view, 10, 50)], [0, 1]);
}

/* ---------- the Entries list: one mark per meaningful state, rows rendered in a window ---------- */

{
  const cmdE = { kind: 'cmd', client: 1, sn: 1 };
  const mine = cmd({ auto: false, injection_round: 10, executed_round: 30 });
  const bg = cmd({ auto: true, injection_round: 10, executed_round: 30 });
  const late = cmd({ auto: true, injection_round: 10, executed_round: 90 });
  assert.equal(entryMark({ kind: 'nop' }, null, 'recovery', 50, null), 'filler');
  assert.equal(entryMark(cmdE, mine, 'recovery', 50, null), 'mine', 'a command you injected');
  assert.equal(entryMark(cmdE, bg, 'recovery', 50, null), 'plain', 'background traffic carries no colour');
  assert.equal(entryMark(cmdE, late, 'recovery', 50, null), 'slow', 'committed later than the run\'s p90');
  // the verify result: a row with a null tally is no certificate; no row is not verified
  const idx = verifyIndex({ round: 150, clients: [{ client: 1, rows: [{ sn: 1, tally: null }, { sn: 2, tally: { accepted: 3, covered: 3 } }] }] });
  assert.deepEqual(certRecord(idx, 1, 1), { round: 150, covered: true, tally: null });
  assert.deepEqual(certRecord(idx, 1, 2), { round: 150, covered: true, tally: { accepted: 3, covered: 3 } });
  assert.deepEqual(certRecord(idx, 1, 3), { round: 150, covered: false, tally: null }, 'committed after the verify round');
  assert.deepEqual(certRecord(idx, 7, 1), { round: 150, covered: false, tally: null }, 'a client the verify does not track');
  assert.equal(certRecord(null, 1, 1), null, 'never verified');
  assert.equal(entryMark(cmdE, mine, 'recovery', 50, certRecord(idx, 1, 1)), 'nocert', 'verified, and no certificate is constructible');
  assert.equal(entryMark(cmdE, mine, 'recovery', 50, certRecord(idx, 1, 2)), 'mine');
  assert.equal(entryMark(cmdE, mine, 'recovery', 50, certRecord(idx, 7, 1)), 'mine', 'not covered by the verify: no claim either way');
  // slow = above the p90 of submit-to-commit latency over committed commands
  const rep = { commands: [10, 20, 30, 40, 50, 60, 70, 80, 90, 100].map(l => cmd({ injection_round: 0, executed_round: l })).concat([cmd()]) };
  assert.equal(slowAbove(rep, 'recovery'), 90);
  // only the rows near the viewport are built
  assert.deepEqual(listWindow(0, 400, 20, 1000, 5), [0, 25]);
  assert.deepEqual(listWindow(2000, 400, 20, 1000, 5), [95, 125]);
  assert.deepEqual(listWindow(19900, 400, 20, 1000, 5), [990, 1000]);
  assert.deepEqual(listWindow(0, 400, 20, 7, 5), [0, 7]);
}

/* ---------- the entry list's selection: commands follow the global pick, fillers stay local ---------- */

{
  const rows = [{ pos: 10, cmd: 4 }, { pos: 11, cmd: -1 }, { pos: 12, cmd: 5 }];
  assert.equal(syncedEntry(rows, 5, null), 12, 'a command selected elsewhere shows here');
  assert.equal(syncedEntry(rows, null, 12), null, 'deselected elsewhere');
  assert.equal(syncedEntry(rows, null, 11), 11, 'a filler pick survives the next render');
  assert.equal(syncedEntry(rows, 99, 11), 11, 'a command outside this block leaves a filler pick alone');
  assert.equal(syncedEntry(rows, 99, 12), null, 'a command picked here loses its pick when another block\'s command is selected');
  // picking: a command toggles the global selection; a filler clears it, so the sync cannot pull it back
  assert.deepEqual(pickedEntry(rows, 4, 10, 11, false), { entry: 11, select: null });
  assert.equal(syncedEntry(rows, null, pickedEntry(rows, 4, 10, 11, false).entry), 11, 'arrow keys pass over a filler');
  assert.deepEqual(pickedEntry(rows, null, null, 12, true), { entry: 12, select: 5 });
  assert.deepEqual(pickedEntry(rows, 5, 12, 12, true), { entry: null, select: null }, 'a second click releases');
  assert.deepEqual(pickedEntry(rows, null, null, 11, true), { entry: 11, select: undefined }, 'nothing global to change');
}

/* ---------- rollbacks belong to the window end before their row ---------- */

{
  const rows = Array.from({ length: 200 }, () => ({ rollbacks: 0, rollback_depth: 0 }));
  rows[160] = { rollbacks: 128, rollback_depth: 2 }; // the row of round 161 carries boundary 160's pass
  assert.deepEqual(rollbackRounds(rows), [{ round: 160, count: 128, depth: 2 }]);
}

/* ---------- a committed-sequence snapshot older than the run says so ---------- */

{
  assert.equal(staleNote('recovery', 120, 120), null);
  assert.equal(staleNote('recovery', 80, 120), 'committed sequence as of r80 · the chain feed stopped', 'recovery\'s feed stops only on an error');
  assert.equal(staleNote('compact', 120, 120), null);
  assert.equal(staleNote('compact', 80, 120), 'committed sequence as of r80 · open a block to refresh');
  assert.equal(staleNote('compact', null, 120), null, 'no sequence at all is reported where it is missing');
}

/* ---------- spread curve ends one round after every log has it ---------- */

{
  const pts = Array.from({ length: 10 }, (_, i) => ({ round: i + 1, useful_holders: Math.min(10, (i + 1) * 2), useful_total: 10 }));
  assert.deepEqual(spreadCurve(cmd({ all_logs_round: 6, spread: pts })).map(p => p.round), [1, 2, 3, 4, 5, 6, 7]);
  const peaked = pts.map(p => (p.round > 6 ? { ...p, useful_holders: 0 } : p));
  assert.deepEqual(spreadCurve(cmd({ all_logs_round: null, spread: peaked })).map(p => p.round), [1, 2, 3, 4, 5], 'still spreading: up to its peak');
}

/* ---------- latency by stage ---------- */

{
  const rep = {
    commands: [
      cmd({ injection_round: 1, delivered_round: 2, prefix_fixed_round: 10, executed_round: 96 }),
      cmd({ injection_round: 1, delivered_round: 4, prefix_fixed_round: 20, executed_round: 96 }),
      cmd({ injection_round: 1, delivered_round: 3, prefix_fixed_round: 30, executed_round: 144 }),
      cmd({ injection_round: 1, delivered_round: 3, prefix_fixed_round: 30, executed_round: null }),
    ],
  };
  const l = stageLatencies(rep, 'recovery', 48, true, null);
  assert.deepEqual(l.map(x => x.key), ['client', 'ordering', 'waiting', 'frozen']);
  assert.deepEqual(l.map(x => [x.median, x.p90, x.n]), [[2, 3, 3], [16, 27, 3], [38, 66, 3], [48, 48, 3]], 'committed commands only');
  assert.deepEqual(stageLatencies(rep, 'recovery', 48, true, new Set([0])).map(x => x.median), [1, 8, 38, 48]);
  assert.deepEqual(stageLatencies(rep, 'recovery', 48, false, null).map(x => x.key), ['client', 'waiting', 'frozen']);
  assert.deepEqual(l.map(x => x.byDefinition), [false, false, false, true], 'recovery\'s frozen stage is T by construction');
}

/* ---------- servers axis ---------- */

{
  const m = (nonbot, useful, blocked) => ({ nonbot_logs: nonbot, useful, blocked });
  const high = { metrics: [m(500, 450, 51), m(510, 440, 60)] };
  assert.deepEqual(serverAxis(high, 512), { lo: 256, cropped: true, half: 256, q3: 384 });
  const low = { metrics: [m(120, 110, 10), m(87, 50, 38)] };
  assert.deepEqual(serverAxis(low, 128), { lo: 0, cropped: false, half: 64, q3: 96 });
  assert.equal(serverAxis({ metrics: [m(512, 512, 300)] }, 512).cropped, false, 'the blocked band counts too');
  assert.equal(serverAxis({ metrics: [m(9, 9, 0)] }, 10).q3, 8, '⌈3n/4⌉');
}

/* ---------- commit margin; recovery's slack to the first checkpoint at or after the deadline ---------- */

{
  assert.equal(slackToCheckpoint(100, 80, 48), 64, 'deadline 100 → checkpoint 144');
  assert.equal(slackToCheckpoint(96, 80, 48), 16, 'a deadline on a boundary is its own checkpoint');
  assert.equal(slackToCheckpoint(100, 150, 48), -6);
  assert.equal(slackToCheckpoint(null, 80, 48), null);
  assert.equal(slackToCheckpoint(100, null, 48), null);

  const cmds = [cmd({ client: 1, delivered_round: 50, prefix_fixed_round: 90 }), cmd({ client: 2, delivered_round: 40, prefix_fixed_round: 101 })];
  const e = c => ({ kind: 'cmd', client: c, sn: 1 });
  const rec = lifelines(cmds, [e(1), e(2)], 48, 'recovery');
  // deadline 98 at both rows (running max), checkpoint 144
  assert.deepEqual(rec.rows.map(r => [r.margin, r.slack]), [[8, 54], [-3, 43]]);
  assert.deepEqual(shownMargin(rec.rows[1], 'recovery'), { value: -3, cls: 'aged', alt: 43 }, 'recovery shows the deadline margin, the slack as alt');
  assert.deepEqual(blockMinMargin(rec.rows, 'recovery'), { value: -3, pos: 2 });
  const comp = lifelines(cmds, [e(1), e(2)], 48, 'compact');
  assert.equal(comp.rows[0].slack, null, 'compact executes continuously: no checkpoint to round up to');
  assert.deepEqual(shownMargin(comp.rows[1], 'compact'), { value: -3, cls: 'aged', alt: null });
  assert.deepEqual(blockMinMargin(comp.rows, 'compact'), { value: -3, pos: 2 });
  assert.equal(blockMinMargin([], 'compact'), null);
}

/* ---------- engine runs ---------- */

const lab = path.resolve(import.meta.dirname, '..');
const context = vm.createContext({ TextDecoder, TextEncoder, WebAssembly, URL, console });
vm.runInContext(fs.readFileSync(path.join(lab, 'sim_wasm.js'), 'utf8'), context);
const wasm = vm.runInContext('wasm_bindgen', context);
context.bytes = fs.readFileSync(path.join(lab, 'sim_wasm_bg.wasm'));
vm.runInContext('wasm_bindgen.initSync({ module: bytes })', context);

const specOf = (proto, extra = {}) => ({
  n: 64, seed: 5, k: 6, ell: 3, sigma: 1.5, proto, injections: [], max_rounds: 300,
  schedule: { kind: 'fresh_per_round', fraction: 0.05 }, traffic: { kind: 'pmf', arrivals_pmf: [0.2, 0.4, 0.4] }, ...extra,
});
function liveRun(spec, rounds, fraction) {
  const live = wasm.LiveSmr.new_with_mode(JSON.stringify(spec), false);
  for (let r = 1; r <= rounds; r++) live.step(typeof fraction === 'function' ? fraction(r) : fraction);
  return JSON.parse(live.report_json());
}

function checkStack(name, rep, proto, T, live) {
  const s = stageSeries(rep, proto, T, live);
  const pending = derive(rep, proto).pending;
  for (let r = 1; r <= rep.metrics.length; r++) {
    const sum = s.keys.reduce((a, k) => a + s.counts[k][r], 0);
    assert.equal(sum, pending[r], `${name}: stage sum = backlog at r${r}`);
  }
  // each command in exactly one stage: its bounds are ordered, so the half-open intervals tile [inj, end)
  const last = lastFrozenP(rep, T);
  rep.commands.forEach((c, i) => {
    const { bounds } = stageBounds(c, proto, T, live, last?.members?.has(i) ? last.boundary : null);
    assert.equal(bounds.length, s.keys.length + 1, `${name}: one bound per stage edge`);
    for (let k = 1; k < bounds.length; k++) assert.ok(bounds[k] >= bounds[k - 1], `${name}: cmd ${i} bounds ordered`);
  });
  return s;
}

for (const [name, spec, fraction, rounds] of [
  ['recovery', specOf({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: false }), 0.05, 300],
  ['recovery+certs', specOf({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: true }), 0.05, 300],
  ['surge', specOf({ kind: 'recovery', t_window_rounds: 40, resend_until_acked: true }, { n: 128, traffic: { kind: 'pmf', arrivals_pmf: [0, 1] } }),
    r => (r >= 41 && r < 111 ? 1 : 0), 300],
]) {
  const rep = liveRun(spec, rounds, fraction);
  const T = spec.proto.t_window_rounds;
  // the rule reproduces every past boundary's P from what executed one window later
  let checked = 0;
  for (let B = T; B + T <= rep.metrics.length; B += T) {
    const truth = rep.commands.map((c, i) => (c.executed_round === B + T ? i : -1)).filter(i => i >= 0);
    assert.deepEqual([...frozenAt(rep, B, T)].sort((a, b) => a - b), truth, `${name}: P(${B}) = what executed at ${B + T}`);
    checked += truth.length;
  }
  assert.ok(checked > 50, `${name}: ${checked} frozen commands checked against executed_round`);
  const last = lastFrozenP(rep, T);
  assert.ok(last && last.members.size > 0, `${name}: the newest P holds commands`);
  assert.ok([...last.members].every(i => rep.commands[i].executed_round == null), `${name}: the newest P has not executed yet`);
  const s = checkStack(name, rep, 'recovery', T, true);
  const N = rep.metrics.length;
  assert.ok(s.counts.frozen[N] >= last.members.size, `${name}: the newest P counts as frozen at the run end`);
  assert.equal(s.counts.frozen[N], last.members.size, `${name}: nothing else is frozen after the last boundary`);
}

{
  const spec = specOf({ kind: 'compact', t_commit_rounds: 24 });
  const rep = liveRun(spec, 200, 0.05);
  const s = checkStack('compact', rep, 'compact', 24, true);
  assert.ok(s.counts.waiting.some(v => v > 0) && s.counts.ordering.some(v => v > 0), 'compact: both live stages populated');
}

{
  const spec = specOf({ kind: 'extended' }, { max_rounds: 120 });
  const rep = liveRun(spec, 120, 0.05);
  checkStack('extended', rep, 'extended', null, true);
}

{
  // a batch report has no position-fixed landmark: the merged stages still sum to the backlog
  const spec = specOf({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: false });
  const rep = JSON.parse(wasm.run_smr_json(JSON.stringify(spec))).report;
  assert.ok(rep.commands.every(c => c.prefix_fixed_round == null), 'batch reports leave position fixed unset');
  const s = checkStack('batch', rep, 'recovery', 24, false);
  assert.deepEqual(s.keys, ['client', 'waiting', 'frozen']);
}

{
  // ordering on a real run: one client issues three commands, the background clients one each
  const spec = specOf({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: false },
    { injections: [{ round: 2, client: 1, op: 1 }, { round: 3, client: 1, op: 2 }, { round: 5, client: 1, op: 3 }] });
  const live = wasm.LiveSmr.new_with_mode(JSON.stringify(spec), false);
  for (let r = 1; r <= 300; r++) live.step(0.05);
  const rep = JSON.parse(live.report_json());
  const order = committedOrder(rep, JSON.parse(live.chain_json(0n)).entries);
  const executed = rep.commands.filter(c => c.executed_round != null).length;
  assert.equal(order.length, executed, 'every executed command sits once in the committed sequence');
  assert.equal(new Set(order).size, order.length);
  order.forEach((i, p) => { if (p) assert.ok(rep.commands[i].executed_round >= rep.commands[order[p - 1]].executed_round, 'commit order follows the executing window ends'); });
  const fifo = fifoCheck(rep, order);
  assert.equal(fifo.multi, 1);
  assert.equal(fifo.pairs, 2);
  assert.equal(fifo.status, 'ok', 'the protocol commits a client\'s commands in issue order');
  const d = displacements(rep, order);
  assert.ok(withinShare(d, 3) > 0.5, `most commands commit near their submission rank (${withinShare(d, 3)})`);
  // the spread cut never runs past one round after every log has it
  for (const c of rep.commands) {
    const pts = spreadCurve(c);
    if (c.all_logs_round != null && pts.length) assert.ok(pts[pts.length - 1].round <= c.all_logs_round + 1);
  }
}

{
  // ends on the first window end that executes: every row reads executed 0, the commands do not
  const spec = specOf({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: false });
  const rep = liveRun(spec, 72, 0.05);
  assert.ok(rep.metrics.every(m => m.max_executed_len === 0), 'the window-end row is read before its pass');
  assert.ok(rep.commands.some(c => c.executed_round === 72));
  assert.equal(checksLabel(safetyChecks(rep, 'recovery', null)), '✓ 7 passed');
}

{
  // marks after a real Verify: compact, two commands by client 1, verified at r150, read at r200
  const spec = specOf({ kind: 'compact', t_commit_rounds: 24 }, { injections: [{ round: 2, client: 1, op: 1 }, { round: 5, client: 1, op: 2 }] });
  const live = wasm.LiveSmr.new_with_mode(JSON.stringify(spec), false);
  for (let r = 1; r <= 150; r++) live.step(0.05);
  const v = JSON.parse(live.verify_json());
  const idx = verifyIndex(v);
  for (let r = 151; r <= 200; r++) live.step(0.05);
  const rep = JSON.parse(live.report_json());
  const entries = JSON.parse(live.certs_json()).committed;
  const at = snIndex(rep.commands);
  const limit = slowAbove(rep, 'compact');
  const nullRows = v.clients.reduce((a, c) => a + c.rows.filter(r => r.tally === null).length, 0);
  let nocert = 0, covered = 0;
  for (const e of entries) {
    const i = e.kind === 'cmd' ? at.get(e.client + ':' + e.sn) ?? -1 : -1;
    const cert = i >= 0 ? certRecord(idx, e.client, e.sn) : null;
    if (cert?.covered) covered++;
    if (entryMark(e, i >= 0 ? rep.commands[i] : null, 'compact', limit, cert) === 'nocert') nocert++;
  }
  assert.ok(entries.length > 100 && covered > 0, `${covered} of ${entries.length} entries covered by the verify`);
  assert.equal(nocert, nullRows, 'red only where the verify found no certificate');
}

console.log('smr_dashboard: ok');
