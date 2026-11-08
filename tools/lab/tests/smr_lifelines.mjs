// Run from the repository root: node tools/lab/tests/smr_lifelines.mjs
// The lifelines view's deadline: a command executes once the whole committed prefix through
// it is T rounds old (compact.rs aged_prefix_len), so deadline(pos) = running max of the
// known stamps at positions 1..pos, plus T. Unit cases first, then the same property on
// engine runs (needs the checked-in sim_wasm build).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { lifelines } from '../smr-derive.js';

const cmd = (client, delivered, extra = {}) => ({
  client, op: 1, auto: false, delivered_round: delivered, all_logs_round: null,
  prefix_fixed_round: null, executed_round: null, committed_round: null, attempts: [], ...extra,
});
const e = (client, sn) => ({ kind: 'cmd', client, sn });

{
  // position 1 lies outside any block shown later; its stamp still bounds position 2
  const cmds = [cmd(1, 10), cmd(2, 3)];
  const out = lifelines(cmds, [e(1, 1), e(2, 1)], 5, 'recovery');
  assert.equal(out.rows[0].dlLo, 15);
  assert.equal(out.rows[1].dlLo, 15, 'running max carries across earlier entries');
  assert.equal(out.rows[1].cmd, 1);
  assert.equal(out.rows[1].delivered, 3);
}

{
  // sn is the command's 1-based order among its client's commands in the report
  const cmds = [cmd(1, 2), cmd(1, 7), cmd(2, 4)];
  const out = lifelines(cmds, [e(1, 2), e(2, 1)], 4, 'recovery');
  assert.equal(out.rows[0].cmd, 1);
  assert.equal(out.rows[0].dlLo, 11);
  assert.equal(out.rows[1].dlLo, 11);
}

{
  // a nop is Nop(round) stamped with its own round; ⊥ and op-less nops carry no known stamp
  const cmds = [cmd(1, 3), cmd(2, 5), cmd(3, 4), cmd(4, 6)];
  const entries = [e(1, 1), { kind: 'nop', op: 20 }, e(2, 1), { kind: 'null', client: 9, sn: 1 }, e(3, 1), { kind: 'nop' }, e(4, 1)];
  const out = lifelines(cmds, entries, 5, 'recovery');
  assert.equal(out.rows[1].kind, 'nop');
  assert.equal(out.rows[1].stamp, 20);
  assert.equal(out.rows[1].dlLo, 25);
  assert.equal(out.rows[2].dlLo, 25, 'the nop stamp raises later deadlines');
  assert.equal(out.rows[3].kind, 'null');
  assert.equal(out.rows[3].stamp, null);
  assert.equal(out.rows[4].dlLo, 25);
  assert.deepEqual(out.skips, { nop: 1, null: 1, cmd: 0 });
  assert.deepEqual(out.rows.map(r => r.skipped), [0, 0, 0, 1, 1, 2, 2], 'skips counted through each position');
}

{
  // no known stamp at all yet: no deadline
  const out = lifelines([cmd(1, 4)], [{ kind: 'null', client: 1, sn: 1 }, e(1, 1)], 5, 'recovery');
  assert.equal(out.rows[0].dlLo, null);
  assert.equal(out.rows[1].dlLo, 9);
}

{
  // a committed entry the report does not know, or a command with no delivery, is skipped
  const out = lifelines([cmd(1, null)], [e(1, 1), e(5, 1)], 5, 'recovery');
  assert.equal(out.rows[0].stamp, null);
  assert.equal(out.rows[1].cmd, -1);
  assert.deepEqual(out.skips, { nop: 0, null: 0, cmd: 2 });
}

{
  // resend: a later accepted copy carries a later stamp, so the deadline is a range;
  // refused attempts do not count, nor (recovery) copies from the boundary that minted the
  // executing P on: executed at 40 with T = 5 means P was minted at 35
  const a = cmd(1, 3, {
    executed_round: 40,
    attempts: [
      { round: 3, target: 1, outcome: 'Delivered' },
      { round: 8, target: 2, outcome: 'TargetBlocked' },
      { round: 12, target: 3, outcome: 'Delivered' },
      { round: 14, target: 4, outcome: 'TargetBot' },
      { round: 35, target: 6, outcome: 'Delivered' },
      { round: 38, target: 5, outcome: 'Delivered' },
    ],
  });
  const b = cmd(2, 4, { attempts: [{ round: 4, target: 1, outcome: 'Delivered' }] });
  const out = lifelines([a, b], [e(1, 1), e(2, 1)], 5, 'recovery');
  assert.equal(out.rows[0].stampHi, 12);
  assert.deepEqual([out.rows[0].dlLo, out.rows[0].dlHi], [8, 17]);
  assert.deepEqual([out.rows[1].dlLo, out.rows[1].dlHi], [9, 17], 'the upper bound is a running max too');
}

{
  // compact accepts a copy as Amplified; Delivered is not a compact outcome
  const a = cmd(1, 3, {
    committed_round: 30,
    attempts: [{ round: 3, outcome: 'Amplified' }, { round: 9, outcome: 'Amplified' }, { round: 11, outcome: 'AckCommitted' },
      { round: 29, outcome: 'Amplified' }, { round: 30, outcome: 'Amplified' }],
  });
  const out = lifelines([a], [e(1, 1)], 6, 'compact');
  assert.deepEqual([out.rows[0].dlLo, out.rows[0].dlHi], [9, 35], 'compact counts copies until the commit round');
  assert.equal(out.rows[0].end, 30, 'compact waits until committed_round');
  const rec = lifelines([cmd(1, 3, { executed_round: 24, committed_round: 99 })], [e(1, 1)], 6, 'recovery');
  assert.equal(rec.rows[0].end, 24, 'recovery waits until executed_round');
}

{
  // margin = lower deadline − prefix_fixed_round; < 0 aged, < T/4 tight, else ok; no fix = unknown
  const fixedAt = [9, 8, 7, 6, null];
  const cmds = fixedAt.map((f, i) => cmd(i + 1, 0, { prefix_fixed_round: f }));
  const out = lifelines(cmds, cmds.map((_, i) => e(i + 1, 1)), 8, 'recovery');
  assert.deepEqual(out.rows.map(r => r.margin), [-1, 0, 1, 2, null]);
  assert.deepEqual(out.rows.map(r => r.cls), ['aged', 'tight', 'tight', 'ok', 'unknown']);
  const odd = lifelines([cmd(1, 0, { prefix_fixed_round: 8 })], [e(1, 1)], 10, 'recovery');
  assert.equal(odd.rows[0].margin, 2);
  assert.equal(odd.rows[0].cls, 'tight', '2 < 10/4');
}

{
  assert.equal(lifelines([], [], 5, 'extended'), null, 'extended has no deadline');
  const cmds = [cmd(1, 2)], entries = [e(1, 1)];
  assert.equal(lifelines(cmds, entries, 5, 'recovery'), lifelines(cmds, entries, 5, 'recovery'), 'cached on identity');
}

/* engine runs: the computed deadline must hold against what the engine actually did */

const lab = path.resolve(import.meta.dirname, '..');
const context = vm.createContext({ TextDecoder, TextEncoder, WebAssembly, URL, console });
vm.runInContext(fs.readFileSync(path.join(lab, 'sim_wasm.js'), 'utf8'), context);
const wasm = vm.runInContext('wasm_bindgen', context);
context.bytes = fs.readFileSync(path.join(lab, 'sim_wasm_bg.wasm'));
vm.runInContext('wasm_bindgen.initSync({ module: bytes })', context);

const protos = {
  compact: { kind: 'compact', t_commit_rounds: 24 },
  recovery: { kind: 'recovery', t_window_rounds: 24, resend_until_acked: false },
  resend: { kind: 'recovery', t_window_rounds: 24, resend_until_acked: true },
};
for (const [name, proto] of Object.entries(protos)) {
  const live = wasm.LiveSmr.new_with_mode(JSON.stringify({
    n: 64, seed: 5, k: 6, ell: 3, sigma: 1.5, proto, injections: [], max_rounds: 400,
    schedule: { kind: 'fresh_per_round', fraction: 0.05 },
    traffic: { kind: 'pmf', arrivals_pmf: [0.2, 0.4, 0.4] },
  }), false);
  for (let i = 0; i < 150; i++) live.step(0.05);
  const rep = JSON.parse(live.report_json());
  // the page's feeds: compact and recovery-with-certificates read certs_json, plain recovery chain_json
  const entries = name === 'recovery' ? JSON.parse(live.chain_json(0n)).entries : JSON.parse(live.certs_json()).committed;
  const T = proto.t_commit_rounds ?? proto.t_window_rounds;
  const out = lifelines(rep.commands, entries, T, proto.kind);
  assert.ok(entries.length > 50, `${name}: the run committed something`);
  let prev = -Infinity, checked = 0;
  out.rows.forEach((r, i) => {
    if (r.dlLo === null) return;
    assert.ok(r.dlLo >= prev, `${name}: deadline non-decreasing at #${i + 1}`);
    assert.ok(r.dlHi >= r.dlLo, `${name}: range ordered at #${i + 1}`);
    prev = r.dlLo;
    if (r.cmd < 0 || r.end === null) return;
    // compact executes the aged prefix the round it ages; recovery mints P at a boundary
    // at or after the deadline and executes it one window later
    const bound = proto.kind === 'compact' ? r.dlLo : r.dlLo + T;
    assert.ok(r.end >= bound, `${name}: #${i + 1} ended r${r.end} before its deadline bound r${bound}`);
    if (proto.kind === 'recovery') assert.equal(r.end % T, 0, `${name}: executed at a boundary`);
    checked++;
  });
  assert.ok(checked > 20, `${name}: ${checked} committed commands checked`);
  const fixed = out.rows.filter(r => r.cmd >= 0 && r.fixed !== null).length;
  assert.ok(fixed >= checked, `${name}: the live engine dates position fixed for every committed command (${fixed} of ${checked})`);
}

console.log('smr_lifelines: ok');
