// Run from the repository root: node tools/lab/tests/smr_pool.mjs
// Live pool sessions: a pool client issues many commands, so every per-command view keys by the
// command (op, or client:sn), never by the client. Unit cases first, then engine runs through the
// checked-in sim_wasm build.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import {
  spreadingHolders, cmdName, cmdChip, clientName, clientModelSpec, poolInFlight, fifoCheck, committedOrder, lifelines,
  chainBlocks, isPool,
} from '../smr-derive.js';

const CB = 2147483648;
const OB = 2 ** 40;
const auto = (slot, j, extra = {}) => ({ client: CB + slot, op: OB + j, auto: true, all_logs_round: null, ...extra });

/* ---------- holders of the selected command ---------- */

{
  // a reused client: its stuck predecessor and its successor spread at once
  const spreading = [{ client: CB + 3, op: OB + 1, holders: [0, 1] }, { client: CB + 3, op: OB + 9, holders: [4] }];
  assert.deepEqual([...spreadingHolders(auto(3, 9), spreading)], [4], 'the successor shows its own holders');
  assert.deepEqual([...spreadingHolders(auto(3, 1), spreading)], [0, 1]);
  assert.equal(spreadingHolders(auto(3, 5, { all_logs_round: 12 }), spreading), 'all', 'done spreading: every log');
  assert.equal(spreadingHolders(auto(3, 5), spreading).size, 0, 'not spreading, not in every log: nobody');
}

/* ---------- names: one per command under both client models ---------- */

{
  assert.equal(cmdChip(auto(7, 7)), 'bg #7', 'unique: arrival index = client slot, unchanged label');
  assert.equal(cmdName(auto(7, 7), false), 'bg #7');
  assert.equal(cmdChip(auto(3, 41)), 'bg #41', 'pool: the arrival index, not the reused slot');
  assert.equal(cmdName(auto(3, 41), true), 'bg #41 · p3');
  assert.equal(cmdName({ client: 5, op: 99, auto: false }, true), 'c5 · op 99');
  assert.equal(cmdChip({ client: 5, op: 99, auto: false }), 'c5·op99');
  assert.equal(clientName(CB + 3), 'p3');
  assert.equal(clientName(5), 'c5');
}

/* ---------- start form: client model ---------- */

{
  assert.equal(clientModelSpec('unique', 64), null, 'unique stays off the wire');
  assert.deepEqual(clientModelSpec('pool', 64), { kind: 'pool', clients: 64 });
  assert.deepEqual(clientModelSpec('pool', 0), { kind: 'pool', clients: 1 }, 'at least one client');
  // in flight ≈ arrival rate × the rounds a command stays a client's open command
  assert.equal(poolInFlight([0.5, 0.5], 'recovery', 16), 16);
  assert.equal(poolInFlight([0, 1], 'compact', 8), 8);
  assert.equal(poolInFlight(null, 'recovery', 16), 0);
}

/* ---------- per-client FIFO on repeated pool clients ---------- */

{
  // report order: (injection_round, client); pool client 0 issues three commands, client 1 two
  const rep = { commands: [auto(0, 0), auto(1, 1), auto(0, 2), auto(1, 3), auto(0, 4)] };
  assert.deepEqual(fifoCheck(rep, [0, 1, 2, 3, 4]), { clients: 2, multi: 2, pairs: 3, violations: 0, status: 'ok' });
  const swapped = fifoCheck(rep, [2, 1, 0, 3, 4]);
  assert.equal(swapped.status, 'fail', 'sn 2 committed ahead of sn 1');
  assert.equal(swapped.violations, 1);
  assert.equal(fifoCheck(rep, [0, 1, 4, 3]).violations, 1, 'sn 3 committed while sn 2 is not');
  assert.equal(fifoCheck(rep, [0, 1, 2, 3, 4], new Set([0, 1])).status, 'na', 'only first commands in scope: nothing tested');
  assert.equal(fifoCheck({ commands: [auto(0, 0), auto(1, 1)] }, [0, 1]).status, 'na');
}

/* ---------- client model detection ---------- */

{
  assert.equal(isPool({ client_model: { kind: 'pool', clients: 8 } }), true);
  assert.equal(isPool({ client_model: { kind: 'unique' } }), false, 'an explicit unique is no pool');
  assert.equal(isPool({}), false);
  assert.equal(isPool(null), false);
  assert.equal(isPool(undefined), false);
}

/* ---------- engine runs ---------- */

const lab = path.resolve(import.meta.dirname, '..');
const context = vm.createContext({ TextDecoder, TextEncoder, WebAssembly, URL, console });
vm.runInContext(fs.readFileSync(path.join(lab, 'sim_wasm.js'), 'utf8'), context);
const wasm = vm.runInContext('wasm_bindgen', context);
context.bytes = fs.readFileSync(path.join(lab, 'sim_wasm_bg.wasm'));
vm.runInContext('wasm_bindgen.initSync({ module: bytes })', context);

const poolSpec = proto => ({
  n: 64, seed: 5, k: 6, ell: 3, sigma: 3, proto, injections: [], max_rounds: 300,
  schedule: { kind: 'fresh_per_round', fraction: 0.05 }, traffic: { kind: 'pmf', arrivals_pmf: [0.5, 0.5] },
  client_model: { kind: 'pool', clients: 64 },
});

{
  const live = wasm.LiveSmr.new_with_mode(JSON.stringify(poolSpec({ kind: 'recovery', t_window_rounds: 24, resend_until_acked: true })), false);
  let sharedRounds = 0;
  for (let r = 1; r <= 300; r++) {
    const st = JSON.parse(live.step(0.05));
    const ops = st.spreading.map(s => s.op);
    assert.equal(new Set(ops).size, ops.length, `round ${r}: one spreading entry per command`);
    if (new Set(st.spreading.map(s => s.client)).size < st.spreading.length) sharedRounds++;
  }
  const rep = JSON.parse(live.report_json());
  const perClient = new Map();
  for (const c of rep.commands) perClient.set(c.client, (perClient.get(c.client) || 0) + 1);
  assert.ok([...perClient.values()].some(k => k >= 3), 'the run reuses pool clients');

  const entries = JSON.parse(live.chain_json(0n)).entries;
  const order = committedOrder(rep, entries);
  const fifo = fifoCheck(rep, order);
  assert.ok(fifo.multi > 0 && fifo.pairs > 0, 'FIFO is applicable on a pool run');
  assert.equal(fifo.status, 'ok', 'the protocol commits each pool client in issue order');

  // Lifelines resolve each committed (client, sn) to the very command the engine committed there
  const ll = lifelines(rep.commands, entries, 24, 'recovery');
  let later = 0;
  ll.rows.forEach((row, i) => {
    const ent = entries[i];
    if (ent.kind !== 'cmd') return;
    assert.ok(row.cmd >= 0, `position ${i + 1} resolves to a command`);
    assert.equal(rep.commands[row.cmd].op, ent.op, `position ${i + 1}: the committed command, not a sibling of its client`);
    if (ent.sn > 1) later++;
  });
  assert.ok(later > 0, 'a reused client commits a later sn');

  // certificates: one card per pool client, one row per sn it issued
  const certs = JSON.parse(live.certs_json());
  const poolCards = certs.clients.filter(c => c.client >= CB);
  assert.ok(poolCards.length > 0);
  for (const c of poolCards) assert.equal(c.certs.length, perClient.get(c.client), `${clientName(c.client)}: one row per sn`);
  // per block: positions come from the whole run, only pairs whose later command sits in the block
  // count, and a block with no such pair tested nothing
  const blocks = chainBlocks(rep, 'recovery', 24).filter(b => b.size);
  const verdicts = blocks.map(b => {
    const scope = new Set(committedOrder(rep, entries.slice(b.from, b.from + b.size)));
    const f = fifoCheck(rep, order, scope);
    if (f.pairs === 0) assert.equal(f.status, 'na', `${b.label}: no pair in scope reads n/a`);
    return f.status;
  });
  assert.ok(blocks.length > 3);
  assert.ok(verdicts.every(v => v !== 'fail'), `every block holds when the run holds: ${verdicts}`);
  assert.ok(verdicts.slice(1).includes('ok'), 'later blocks check real pairs');
  assert.ok(verdicts.includes('na'), 'the first blocks have no pair in scope');

  console.log(`pool run: ${rep.commands.length} commands over ${perClient.size} clients, FIFO ${fifo.pairs} pairs ok, ${sharedRounds} rounds with a reused client spreading twice`);
}

console.log('smr_pool: ok');
