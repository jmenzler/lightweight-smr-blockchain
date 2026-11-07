// Run from the repository root: node tools/lab/tests/smr_delta.mjs
// Replays engine-produced report deltas through the page's real applyReportDelta and compares
// with the engine's own full report after every step (needs the checked-in sim_wasm build).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { applyReportDelta, EMPTY_REPORT } from '../smr-delta.js';

const lab = path.resolve(import.meta.dirname, '..');
const context = vm.createContext({ TextDecoder, TextEncoder, WebAssembly, URL, console });
vm.runInContext(fs.readFileSync(path.join(lab, 'sim_wasm.js'), 'utf8'), context);
const wasm = vm.runInContext('wasm_bindgen', context);
context.bytes = fs.readFileSync(path.join(lab, 'sim_wasm_bg.wasm'));
vm.runInContext('wasm_bindgen.initSync({ module: bytes })', context);

const protos = {
  extended: { kind: 'extended' },
  compact: { kind: 'compact', t_commit_rounds: 6 },
  recovery: { kind: 'recovery', t_window_rounds: 10, resend_until_acked: false },
  resend: { kind: 'recovery', t_window_rounds: 10, resend_until_acked: true },
};
const spec = (proto, seed, injections = [{ round: 2, client: 1, op: 7 }]) => JSON.stringify({
  n: 32, seed, k: 6, ell: 3, sigma: 1.5, proto: protos[proto], injections, max_rounds: 400,
  schedule: { kind: 'fresh_per_round', fraction: 0.1 },
  traffic: { kind: 'pmf', arrivals_pmf: [0.3, 0.4, 0.3] },
});

function session(proto, seed, injections) {
  const live = wasm.LiveSmr.new_with_mode(spec(proto, seed, injections), false);
  let page = null;
  let steps = 0;
  const sync = label => {
    const next = applyReportDelta(page ?? EMPTY_REPORT, JSON.parse(live.report_delta_json()));
    if (page) {
      assert.notEqual(next, page, 'a merge returns a new report');
      assert.notEqual(next.commands, page.commands, 'a merge returns a new commands array');
      assert.notEqual(next.metrics, page.metrics, 'a merge returns a new metrics array');
    }
    const before = page && { metrics: page.metrics.length, commands: page.commands.length };
    page = next;
    if (before) assert.ok(before.metrics <= page.metrics.length && before.commands <= page.commands.length);
    assert.deepStrictEqual(page, JSON.parse(live.report_json()), `${proto} ${label} after ${steps} steps`);
  };
  return {
    live,
    sync,
    step(fraction, count = 1) {
      for (let i = 0; i < count; i++) { live.step(fraction); steps++; }
    },
    get page() { return page; },
  };
}

for (const proto of Object.keys(protos)) {
  const s = session(proto, 3);
  s.sync('start');
  for (let i = 0; i < 40; i++) { s.step(0.1); s.sync('every step'); }
  s.live.inject(5, 99n, undefined);
  s.live.set_traffic(JSON.stringify([0, 0, 0, 0, 0, 0, 1]));
  for (let i = 0; i < 20; i++) { s.step(0.4); s.sync('surge'); }
  for (let i = 0; i < 3; i++) { s.step(0.1, 25); s.sync('25-step span'); }
}

{
  const s = session('compact', 1, [{ round: 2, client: 1, op: 7 }, { round: 30, client: 2, op: 8 }]);
  s.sync('start');
  s.step(0.1, 5);
  s.sync('before the live inject');
  s.live.inject(3, 99n, undefined);
  s.sync('after the live inject');
  const ops = s.page.commands.map(c => c.op);
  assert.ok(ops.indexOf(7) < ops.indexOf(99) && ops.indexOf(99) < ops.indexOf(8), 'the live command lands before the round-30 command');
}

{
  const s = session('resend', 1);
  s.sync('start');
  s.step(0.3, 30);
  s.sync('catch up');
  const before = JSON.parse(s.live.report_delta_json());
  assert.deepEqual([before.inserted, before.changed, before.metrics], [[], [], []], 'nothing new, nothing sent');
  assert.throws(() => applyReportDelta(EMPTY_REPORT, { ...before, rounds_from: 30 }), /starts at round 30, the page holds 0/);
  s.step(0.3);
  const next = JSON.parse(s.live.report_delta_json());
  assert.throws(() => applyReportDelta(EMPTY_REPORT, next), /starts at round 30, the page holds 0/, 'a delta on the wrong base throws');
  assert.throws(() => applyReportDelta(s.page, { ...next, rec_rows_from: 0 }), /recovery row/, 'recovery cursor is checked');
  assert.throws(() => applyReportDelta(s.page, { ...next, len: s.page.commands.length + 5 }), /report delta/, 'a length that does not add up throws');
}

{
  const old = { a: 1 };
  const command = { op: 1, committed_round: 4 };
  const prev = { metrics: [], commands: [command, old], terminal: 'Ran' };
  const gone = { op: 1 };
  const next = applyReportDelta(prev, {
    rounds_from: 0, metrics: [], rec_rows_from: 0, rec_rows: [], summary: { terminal: 'Ran' },
    len: 2, inserted: [], changed: [[0, gone]], appended: [],
  });
  assert.equal(next.commands[0], gone, 'a changed command replaces the old one whole');
  assert.equal('committed_round' in next.commands[0], false, 'a field that vanished is gone');
  assert.equal(next.commands[1], old);
  assert.equal(prev.commands[0], command, 'the previous report is untouched');
}

{
  const spread = [{ round: 1 }];
  const attempts = [{ round: 1, target: 3 }];
  const command = { op: 1, status: 'Pending', spread, attempts };
  const prev = { metrics: [], commands: [command] };
  const base = { rounds_from: 0, metrics: [], rec_rows_from: 0, rec_rows: [], summary: {}, len: 1, inserted: [], changed: [] };
  const next = applyReportDelta(prev, {
    ...base,
    appended: [[0, { op: 1, status: 'Complete' }, 1, [{ round: 2 }], 1, []]],
  });
  assert.deepEqual(next.commands[0], { op: 1, status: 'Complete', spread: [{ round: 1 }, { round: 2 }], attempts });
  assert.notEqual(next.commands[0].spread, spread, 'the grown curve is a new array');
  assert.equal(spread.length, 1, 'the old curve is untouched');
  assert.equal(command.status, 'Pending');
  assert.throws(() => applyReportDelta(prev, { ...base, appended: [[0, { op: 1 }, 5, [], 1, []]] }), /spread/, 'a curve cursor mismatch throws');
  assert.throws(() => applyReportDelta(prev, { ...base, appended: [[0, { op: 1 }, 1, [], 0, []]] }), /attempts/, 'an attempts cursor mismatch throws');
  assert.throws(() => applyReportDelta(prev, { ...base, appended: [[3, { op: 1 }, 1, [], 1, []]] }), /outside/, 'an index outside the commands throws');
}

console.log('smr delta tests passed');
