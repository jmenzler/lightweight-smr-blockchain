// Run from the repository root: node --experimental-vm-modules tools/lab/tests/smr_failure.mjs
// (the flag is required: the lab modules are loaded through node:vm).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { Element } from './dom-stub.mjs';

const root = path.resolve(import.meta.dirname, '../../..');
const lab = path.join(root, 'tools/lab');

const failure = {
  node: 7,
  attempted_round: 100,
  completed_round: 99,
  observed_round: 100,
  phase: 'BoundaryPreflight',
  violation: {
    pre_len: 47,
    log_len: 119,
    first_mismatch: 41,
    expected: null,
    actual: null,
    command_prefix_ok: false,
  },
};
const regressionFailure = {
  ...failure,
  phase: 'ExecutedRegression',
  violation: { previous_len: 52, len: 47 },
};
const snOrderFailure = {
  ...failure,
  phase: 'OutOfSnOrderCommit',
  violation: { client: 5, sn: 3, committed_sn: 1 },
};
const failedTerminal = {
  Failed: {
    attempted_round: 100,
    completed_round: 99,
    observed_round: 100,
    phase: 'BoundaryPreflight',
  },
};
const metric = round => ({
  round,
  blocked: 0,
  nonbot_logs: 598,
  useful: 598,
  max_log_len: 119,
  min_executed_len: 47,
  max_executed_len: 47,
  lcp_len: 47,
});
const recoveryRow = round => ({
  noreset: 598,
  reset: 0,
  bot_r: 0,
  window: Math.floor(round / 20),
  rollbacks: 0,
  max_checkpoint_window: 5,
  rollback_depth: 0,
  cp_fork_k: 1,
  max_cp_p_len: 47,
});
const report = (tag, failed = false) => ({
  tag,
  metrics: failed ? Array.from({ length: 100 }, (_, i) => metric(i + 1)) : [],
  commands: [],
  terminal: failed ? failedTerminal : { Ran: { rounds: 200 } },
  safety_ok: true,
  ...(failed ? {
    failure: { regression: regressionFailure, sn_order: snOrderFailure }[failed] ?? failure,
    recovery: { fork_ok: true, rounds: Array.from({ length: 100 }, (_, i) => recoveryRow(i + 1)) },
  } : {}),
});
const certs = () => ({ servers: [], committed: [], clients: [], merges_last_step: [], roots_consistent: true });

function workerApi(owner) {
  let panic = null;
  return {
    initSync() {},
    last_panic: () => panic,
    run_smr_json(json) {
      const spec = JSON.parse(json);
      owner.lastSpec = spec;
      if (owner.panic) {
        panic = owner.panic;
        throw new WebAssembly.RuntimeError('unreachable');
      }
      const result = report(`batch-${spec.seed}`, owner.batchFailed);
      if (owner.batchFailed && owner.failedRelease) {
        result.metrics[0].blocked = spec.n;
        result.metrics[1].blocked = 0;
      }
      return JSON.stringify({ report: result });
    },
    LiveSmr: {
      new_with_mode(json) {
        const spec = JSON.parse(json);
        owner.lastSpec = spec;
        let round = 0;
        const fractions = [];
        let failed = false;
        const counts = owner.calls;
        return {
          free() {},
          step(fraction) {
            if (owner.panic) {
              panic = owner.panic;
              throw new WebAssembly.RuntimeError('unreachable');
            }
            if (failed) return JSON.stringify({ round, failure, absorbed: true });
            round++;
            fractions.push(fraction);
            failed = owner.liveFailsAt === round;
            const dead = owner.liveDeadAt !== null && round >= owner.liveDeadAt ? owner.liveDeadAt : null;
            return JSON.stringify({ round, failure: failed ? failure : null, absorbed: false, dead, nodes: [], blocked: [], spreading: [] });
          },
          report_json() { counts.report++; return JSON.stringify(report(`live-${spec.seed}-r${round}`, failed)); },
          report_delta_json() {
            counts.delta++;
            const { metrics, commands, ...summary } = report(`live-${spec.seed}-r${round}`);
            return JSON.stringify({ rounds_from: owner.badDelta ? 5 : 0, metrics, rec_rows_from: 0, rec_rows: [], summary, len: 0, inserted: [], changed: [], appended: [] });
          },
          export_scenario() {
            counts.export++;
            return JSON.stringify({ ...spec, max_rounds: round, schedule: { kind: 'per_round_fractions', fractions: [...fractions] } });
          },
          certs_json() { counts.certs++; return JSON.stringify(certs()); },
          chain_json() { counts.chain++; return JSON.stringify({ m: 0, entries: [], peaks: [], merges: [] }); },
          inject() { counts.mutate++; return '{}'; },
          set_traffic() { counts.mutate++; return '{}'; },
          set_block() { counts.mutate++; return '{}'; },
          node_detail() { counts.detail++; return '{}'; },
          capture_stale() { counts.detail++; return '{}'; },
          verify_json() { counts.detail++; return '{}'; },
        };
      },
    },
  };
}

async function setup() {
  const elements = new Map();
  const $ = id => {
    if (!elements.has(id)) elements.set(id, new Element(id));
    return elements.get(id);
  };
  for (const [id, value] of Object.entries({
    smrN: '598', smrSeed: '1', smrSigma: '1', smrMax: '200', smrBeta: '10',
    smrT: '8', smrTw: '20', smrStrategy: 'fresh', smrArrRate: '',
    smrAuditScope: 'all', smrSlider: '30', smrSpeed: '1',
  })) $(id).value = value;
  $('smrAbort').classList.add('hidden');
  const S = { engine: 'loading', view: 'smr', live: null };
  const downloads = [];
  const drawer = { open: false };
  const workers = [];
  const timers = new Map();
  let timerId = 0;
  const lists = new Map([['#smrSpeed button', ['1', 'max'].map(speed => {
    const b = new Element('speed-' + speed);
    b.dataset.speed = speed;
    return b;
  })]]);
  const windowListeners = {};
  const document = {
    getElementById: $,
    querySelectorAll: query => lists.get(query) ?? [],
    addEventListener() {},
    createElement: tag => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
  };
  const context = vm.createContext({
    console,
    document,
    window: { devicePixelRatio: 1, addEventListener: (type, fn) => (windowListeners[type] ??= []).push(fn) },
    Event: class { constructor(type) { this.type = type; } },
    ResizeObserver: class { observe() {} },
    setTimeout: fn => { const id = ++timerId; timers.set(id, fn); return id; },
    clearTimeout: id => timers.delete(id),
  });

  class Worker {
    constructor() {
      this.terminated = false;
      this.outbox = [];
      this.batchFailed = false;
      this.failedRelease = false;
      this.liveFailsAt = null;
      this.liveDeadAt = null;
      this.panic = null;
      this.badDelta = false;
      this.hold = false;
      this.held = [];
      this.lastSpec = null;
      this.calls = { report: 0, delta: 0, export: 0, certs: 0, chain: 0, mutate: 0, detail: 0 };
      workers.push(this);
      const workerContext = vm.createContext({
        self: { location: { search: '' } },
        importScripts() {},
        wasm_bindgen: workerApi(this),
        fetch: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
        postMessage: message => {
          this.outbox.push(message);
          if (this.hold) this.held.push(message);
          else if (!this.terminated) this.onmessage?.({ data: message });
        },
        performance,
        console,
        WebAssembly,
      });
      vm.runInContext(fs.readFileSync(path.join(lab, 'worker.js'), 'utf8'), workerContext);
      this.workerContext = workerContext;
    }
    postMessage(message) { this.workerContext.onmessage({ data: message }); }
    release() {
      this.hold = false;
      for (const message of this.held.splice(0)) {
        if (!this.terminated) this.onmessage?.({ data: message });
      }
    }
    terminate() { this.terminated = true; }
  }
  context.Worker = Worker;

  const stubs = {
    'state.js': { S, $, PALETTE: ['#000'], AUTO_CLIENT_BASE: 2147483648 },
    'form.js': { downloadJson: (name, json) => downloads.push({ name, data: JSON.parse(json) }) },
    'pmf-editor.js': { createPmfEditor: () => ({ weights: () => [1], draw() {} }) },
    'smr-certs.js': {
      onCertsSnapshot() {}, onChainSnapshot() {}, freezeChain() {},
      useEntrySelect() {}, resetCertsDrawer() {}, onVerifyTallies() {},
      markCertsLoading() {}, endCertsLoading() {},
    },
    'smr-viewport.js': { renderViewport() {}, resetViewport() {} },
    'smr-drawer.js': {
      renderDrawer() {}, resetDrawer() {}, useBlockDrawerOpen() {}, blockDrawerOpen: () => drawer.open, onMerkleTree() {},
    },
    'render.js': { update() {}, draw() {}, setView() {} },
    'trace.js': { loadTrace() {} },
    'live.js': { onBatchDone() {}, onLiveReady() {}, onLiveRound() {}, onLiveScenario() {}, onLiveTraceJson() {} },
    'controls.js': {},
    'tooltip.js': {},
    'chrome.js': {},
    'spill.js': { onSpillReady() {}, onSpillChain() {} },
  };
  const modules = new Map();
  const load = requested => {
    const name = path.basename(requested);
    if (modules.has(name)) return modules.get(name);
    const module = stubs[name]
      ? new vm.SyntheticModule(Object.keys(stubs[name]), function () {
          for (const [key, value] of Object.entries(stubs[name])) this.setExport(key, value);
        }, { context, identifier: name })
      : new vm.SourceTextModule(fs.readFileSync(path.join(lab, name), 'utf8'), { context, identifier: name });
    modules.set(name, module);
    return module;
  };
  const main = load('main.js');
  await main.link(load);
  await main.evaluate();
  const flush = async () => { for (let i = 0; i < 16; i++) await Promise.resolve(); };
  await flush();
  const key = k => {
    const event = { key: k, target: { tagName: 'BODY' }, prevented: false, preventDefault() { this.prevented = true; } };
    for (const fn of windowListeners.keydown || []) fn(event);
    return event;
  };
  return { S, $, lists, key, downloads, drawer, workers, timers, flush, smr: modules.get('smr.js').namespace };
}

const hidden = element => element.classList.contains('hidden');
const batchSpec = (overrides = {}) => ({
  n: 598, seed: 1, k: 6, ell: 3, sigma: 1, proto: { kind: 'compact', t_commit_rounds: 8 },
  injections: [], max_rounds: 200, schedule: { kind: 'fresh_per_round', fraction: 0.1 }, ...overrides,
});
const recoverySpec = (overrides = {}) => batchSpec({ proto: { kind: 'recovery', t_window_rounds: 20, resend_until_acked: true }, ...overrides });

{
  const h = await setup();
  h.workers[0].batchFailed = 'regression';
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assert.match(h.$('smrAbort').innerHTML, /executed length fell from 52 to 47/);
  assert.doesNotMatch(h.$('smrAbort').innerHTML, /undefined/);
}

{
  const h = await setup();
  h.workers[0].batchFailed = 'sn_order';
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assert.match(h.$('smrAbort').innerHTML, /client 5 committed sn 3 while its committed sn was 1/);
  assert.doesNotMatch(h.$('smrAbort').innerHTML, /before mutating boundary/);
  assert.doesNotMatch(h.$('smrAbort').innerHTML, /undefined/);
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.batchFailed = true;
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assert.equal(worker.terminated, false, 'typed batch failure keeps the worker');
  assert.equal(h.workers.length, 1, 'typed batch failure does not respawn');
  assert.equal(h.S.smr.report.failure.node, 7);
  assert.equal(h.S.smr.scenario.seed, 1);
  assert.match(h.$('smrStatus').textContent, /FAILED.*attempted 100.*completed 99.*observed 100/i);
  assert.equal(hidden(h.$('smrAbort')), false);
  assert.match(h.$('smrAbort').innerHTML, /prefix 47, log 119, first mismatch 41/);

  h.$('smrSeed').value = '99';
  h.$('smrExportSpec').click();
  h.$('smrExportReport').click();
  assert.equal(h.downloads[0].data.seed, 1, 'failure export uses captured input, not edited form');
  assert.equal(h.downloads[1].data.failure.node, 7);
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.batchFailed = true;
  worker.hold = true;
  h.smr.runSpecBatch(recoverySpec());
  await h.flush();

  h.S.smr.proto = 'compact';
  h.$('smrSeed').value = '99';
  h.$('smrT').value = '8';
  worker.release();
  await h.flush();

  assert.equal(h.S.smr.proto, 'compact', 'failed display must not overwrite editable next-run config');
  assert.match(h.$('smrSumChip').textContent, /598 servers.*recovery.*T 20.*seed 1/);
  assert.equal(hidden(h.$('smrRecChips')), false, 'recovery readout is chosen by the captured protocol, not the edited form');

  h.$('smrSeed').value = '77';
  h.$('smrExportSpec').click();
  assert.equal(h.downloads.at(-1).data.seed, 1, 'post-failure edits do not change captured export');
  assert.equal(h.downloads.at(-1).data.proto.kind, 'recovery');
  assert.equal(h.downloads.at(-1).data.proto.t_window_rounds, 20);

  worker.batchFailed = false;
  h.$('smrStartBtn').click();
  await h.flush();
  assert.equal(worker.lastSpec.seed, 77, 'explicit next run uses edited form');
  assert.equal(worker.lastSpec.proto.kind, 'compact');
  assert.equal(h.S.smr.report.tag, 'live-77-r0');
  assert.equal(hidden(h.$('smrAbort')), true);
}

{
  const h = await setup();
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assert.equal(h.S.smr.report.tag, 'batch-1');
  h.workers[0].batchFailed = true;
  h.smr.runSpecBatch(batchSpec({ seed: 2 }));
  await h.flush();
  assert.equal(h.S.smr.report.tag, 'batch-2', 'new failed report replaces prior success');
  assert.equal(h.S.smr.report.failure.node, 7);
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.liveFailsAt = 2;
  h.$('smrStartBtn').click();
  await h.flush();
  h.$('smrSlider').value = '30';
  h.$('smrStepBtn').click();
  await h.flush();
  h.$('smrSlider').value = '40';
  h.$('smrStepBtn').click();
  await h.flush();

  assert.equal(h.S.smr.live, null, 'failed live session cannot resume');
  assert.equal(JSON.stringify(h.S.smr.scenario.schedule.fractions), '[0.3,0.4]');
  assert.equal(worker.calls.certs, 0, 'a closed drawer reads no certificates');
  assert.equal(worker.calls.chain, 0, 'a closed drawer reads no chain');
  assert.equal(worker.calls.delta, 2, 'the initial report and the first step arrive as deltas');
  assert.equal(worker.calls.report, 1, 'only the failing step builds a full report');
  assert.equal(worker.calls.export, 1, 'failed replay captured exactly once');
  assert.equal(h.workers.length, 1, 'typed live failure does not respawn');
  assert.equal(h.timers.size, 0, 'failed session leaves no autoplay callback');

  h.$('smrSeed').value = '88';
  h.$('smrExportSpec').click();
  assert.equal(JSON.stringify(h.downloads.at(-1).data.schedule.fractions), '[0.3,0.4]');

  worker.liveFailsAt = null;
  h.$('smrStartBtn').click();
  await h.flush();
  assert.ok(h.S.smr.live, 'explicit new session starts cleanly');
  assert.equal(hidden(h.$('smrAbort')), true, 'new session clears failure banner');
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.liveFailsAt = 2;
  h.$('smrStartBtn').click();
  await h.flush();
  h.$('smrPlayBtn').click();
  await h.flush();
  assert.equal(h.S.smr.lastStatus.round, 1);
  assert.ok(h.timers.size > 0, 'autoplay queued work for the first session');

  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(h.S.smr.live, null, 'manual step reached typed failure');
  const stale = [...h.timers.values()];
  h.timers.clear();

  worker.liveFailsAt = null;
  h.$('smrStartBtn').click();
  await h.flush();
  assert.equal(h.S.smr.lastStatus, null);
  assert.equal(h.S.smr.live.playing, false);
  for (const callback of stale) callback();
  await h.flush();
  assert.equal(h.S.smr.lastStatus, null, 'old autoplay must not advance the new session');

  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(h.S.smr.lastStatus.round, 1, 'manual stepping remains available');
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.batchFailed = true;
  worker.failedRelease = true;
  worker.hold = true;
  h.smr.runSpecBatch(recoverySpec());
  await h.flush();
  worker.release();
  await h.flush();
  assert.doesNotMatch(h.$('smrPhaseChip').textContent, /CONFIRMED/);
  assert.doesNotMatch(h.$('smrClockChip').textContent, /r\* 20|R 18/);
  assert.equal(hidden(h.$('smrLawCard')), true);
}

{
  const h = await setup();
  const worker = h.workers[0];
  worker.liveFailsAt = 3;
  h.$('smrStartBtn').click();
  await h.flush();
  assert.equal(worker.calls.certs, 0, 'a new session reads no certificates');
  h.drawer.open = true;
  h.$('smrStepBtn').click();
  await h.flush();
  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(worker.calls.certs, 2, 'with the block drawer open every step reads the certificates');
  assert.equal(worker.calls.chain, 0, 'a certificate session has no chain feed');
  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(h.S.smr.live, null);
  assert.equal(worker.calls.certs, 2, 'the failing step skips the certificate read');
  assert.equal(worker.calls.chain, 0, 'the failing step skips the chain read');
}

{
  const h = await setup();
  const worker = h.workers[0];
  h.$('smrStartBtn').click();
  await h.flush();
  const seen = () => worker.outbox.at(-1);
  worker.postMessage({ type: 'smr-live-step', fraction: 0.1, wantCerts: false, wantChain: true });
  await h.flush();
  assert.equal(worker.calls.chain, 1, 'wantChain reads the chain delta');
  assert.equal(worker.calls.certs, 0);
  assert.ok(seen().chain && seen().certs === undefined);
  worker.postMessage({ type: 'smr-live-feeds', wantCerts: true, wantChain: false });
  await h.flush();
  assert.equal(seen().type, 'smr-feeds');
  assert.equal(worker.calls.certs, 1, 'the drawer-open request reads the certificates');
  assert.equal(worker.calls.chain, 1);
  worker.postMessage({ type: 'smr-live-step', fraction: 0.1 });
  await h.flush();
  assert.equal(worker.calls.certs + worker.calls.chain, 2, 'a step without flags reads neither');
}

{
  const h = await setup();
  const worker = h.workers[0];
  h.$('smrStartBtn').click();
  await h.flush();
  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(h.S.smr.report.tag, 'live-1-r1', 'the step delta rebuilt the report');
  h.$('smrExportReport').click();
  await h.flush();
  assert.equal(worker.calls.report, 1, 'a live export asks the engine for its report');
  assert.equal(h.downloads.at(-1).data.tag, 'live-1-r1');

  worker.badDelta = true;
  h.$('smrStepBtn').click();
  await h.flush();
  assert.equal(h.S.smr.live, null, 'a delta that does not fit ends the session');
  assert.equal(hidden(h.$('smrAbort')), false);
  assert.match(h.$('smrAbort').innerHTML, /lost sync.*starts at round 5, the page holds 0/);
  assert.equal(h.S.smr.report.tag, 'live-1-r1', 'the last good report stays on screen');
}

{
  const h = await setup();
  const oldWorker = h.workers[0];
  oldWorker.panic = 'unexpected panic';
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assert.equal(oldWorker.terminated, true);
  assert.equal(h.workers.length, 2, 'unexpected panic still respawns');
}

console.log('smr failure adapter tests passed');

const PLAY_CONTROLS = ['smrPlayBtn', 'smrStepBtn', 'smrJumpBtn'];
function assertStopped(h, text, label) {
  assert.equal(h.S.smr.live, null);
  assert.equal(h.$('smrStatus').textContent, text);
  assert.match(h.$('smrLiveRound').textContent, label);
  for (const id of PLAY_CONTROLS) assert.equal(h.$(id).disabled, true, `${id} greys out`);
  for (const b of h.lists.get('#smrSpeed button')) assert.equal(b.disabled, true, 'speed greys out');
  assert.ok(h.$('smrPlayBtn').dataset.tip.startsWith(text), 'the greyed Play says why');
  assert.equal(hidden(h.$('smrNewBtn')), false, 'New run stays');
  assert.equal(h.$('smrExportBtn').disabled, undefined, 'Export stays');
  h.$('smrStatus').textContent = 'verify: something else';
  const e = h.key(' ');
  assert.ok(e.prevented, 'space on a stopped run is handled');
  assert.match(h.$('smrStatus').textContent, new RegExp('^' + text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&') + ' · New run starts another'));
  assert.ok(h.$('smrStatus').classes.has('flash'), 'the reminder flashes');
}

{
  const h = await setup();
  h.$('smrMax').value = '3';
  h.$('smrStartBtn').click();
  await h.flush();
  assert.equal(h.$('smrPlayBtn').disabled, false);
  assert.equal(h.$('smrPlayBtn').dataset.tip, undefined, 'a running session has no end tooltip');
  for (let i = 0; i < 3; i++) { h.$('smrStepBtn').click(); await h.flush(); }
  assertStopped(h, 'finished · 3/3 rounds', /round 3\/3 · finished/);

  h.$('smrNewBtn').click();
  h.$('smrMax').value = '200';
  h.$('smrStartBtn').click();
  await h.flush();
  assert.equal(h.S.smr.end, null, 'a new session clears the end state');
  assert.equal(h.$('smrPlayBtn').dataset.tip, undefined);
  for (const b of h.lists.get('#smrSpeed button')) assert.equal(b.disabled, false);
}

{
  const h = await setup();
  h.workers[0].liveDeadAt = 2;
  h.$('smrStartBtn').click();
  await h.flush();
  for (let i = 0; i < 2; i++) { h.$('smrStepBtn').click(); await h.flush(); }
  assertStopped(h, '⚠ finished · every server ⊥ at round 2/200 — nothing can recover', /round 2\/200 · all ⊥/);
}

{
  const h = await setup();
  h.$('smrStartBtn').click();
  await h.flush();
  h.$('smrStepBtn').click();
  await h.flush();
  h.$('smrEndBtn').click();
  assertStopped(h, 'ended · stopped at round 1/200 by End', /round 1\/200 · ended/);
}

{
  const h = await setup();
  h.smr.runSpecBatch(batchSpec());
  await h.flush();
  assertStopped(h, 'finished · 200/200 rounds', /round 200\/200 · finished/);
}

console.log('smr end-state tests passed');

