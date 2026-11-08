// Run from the repository root: node --experimental-vm-modules tools/lab/tests/sv_end.mjs
// Drives the single-value session (live.js) with a stubbed engine and checks that every
// way a run stops leaves an explicit end state: status, round label, controls, keys.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { Element } from './dom-stub.mjs';

const lab = path.resolve(import.meta.dirname, '..');

async function setup(max = 200) {
  const elements = new Map();
  const $ = id => {
    if (!elements.has(id)) elements.set(id, new Element(id));
    return elements.get(id);
  };
  const S = { trace: null, t: 0, playing: false, speed: 1, sel: null, view: 'single', series: null, engine: 'wasm', live: null,
    sv: { ctx: null, pending: null, handle: false, speed: 1, program: null, srvOpen: true, end: null } };
  const posted = [];
  const spec = { n: 4, seed: 1, k: 6, ell: 3, init: { kind: 'split', fraction: 0.5 }, max_rounds: max, schedule: { kind: 'fresh_per_round', fraction: 0 } };
  const lists = new Map([['#svSpeed button', ['1', 'max'].map(speed => {
    const b = new Element('speed-' + speed);
    b.dataset.speed = speed;
    return b;
  })]]);
  const context = vm.createContext({
    console,
    document: { getElementById: $, querySelectorAll: q => lists.get(q) ?? [], addEventListener() {} },
    setTimeout: () => 0, setInterval: () => 0, clearInterval() {},
  });
  const stubs = {
    'state.js': { S, $, AUTO_CLIENT_BASE: 2147483648 },
    'engine.js': { post: m => posted.push(m) },
    'render.js': { update() {} },
    'trace.js': { loadTrace(tr) { S.trace = tr; S.series = { values: [], vIndex: {}, comp: [] }; } },
    'form.js': { buildSpec: () => spec, validateSpec: () => null, strategy: () => 'fresh', readProgram: () => null, LIVE_STRATEGIES: ['fresh', 'sticky', 'program'], downloadJson() {} },
    'sv-viewport.js': { useSeek() {}, resetSvViewport() {} },
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
  const live = load('live.js');
  await live.link(load);
  await live.evaluate();
  return { S, $, posted, spec, lists, live: live.namespace };
}

const roundMsg = (round, extra = {}) => ({ round, states: [0, 0, 1, 1], targets: [], blocked: [], agreed_value: null, all_hold: false, all_undecided: false, absorbed: false, ...extra });
async function startLive(h) {
  h.$('svStartBtn').click();
  h.live.onLiveReady({ json: JSON.stringify({ n: 4, seed: 1, initial: [0, 0, 1, 1], rounds: [] }) });
}

function assertFinished(h, text, label) {
  assert.equal(h.S.live, null);
  assert.equal(h.$('runStatus').textContent, text);
  assert.match(h.$('svRoundLbl').textContent, label);
  assert.equal(h.$('svStepBtn').disabled, true, '+1 greys out at the end of a finished run');
  assert.ok(h.$('svStepBtn').dataset.tip.startsWith(text), 'the greyed +1 says why');
  assert.equal(h.$('svPlayBtn').textContent, '⏵ Replay', 'Play on a finished run replays it');
  assert.notEqual(h.$('svPlayBtn').disabled, true, 'replay stays available');
  assert.equal(h.$('svNewBtn').classes.has('hidden'), false);
  h.$('runStatus').textContent = 'something else';
  h.live.stepForward();
  assert.match(h.$('runStatus').textContent, / · Replay plays it again; New run starts another$/);
  assert.ok(h.$('runStatus').textContent.startsWith(text), 'a step past the end re-states why');
  assert.ok(h.$('runStatus').classes.has('flash'));
}

{
  const h = await setup(2);
  await startLive(h);
  assert.equal(h.$('svStepBtn').disabled, false);
  h.live.onLiveRound(roundMsg(1));
  h.live.onLiveRound(roundMsg(2));
  assertFinished(h, 'finished · 2/2 rounds', /round 2\/2 · finished/);
}

{
  const h = await setup();
  await startLive(h);
  h.live.onLiveRound(roundMsg(1, { states: [0, 0, 0, 0], agreed_value: 0, all_hold: true }));
  assert.ok(h.S.live, 'agreement at 0% blocking pauses, the slider can still stress it');
  assert.match(h.$('runStatus').textContent, /^agreement on 0 at round 1 · paused/);
  h.live.onLiveRound(roundMsg(1, { states: [0, 0, 0, 0], agreed_value: 0, all_hold: true, absorbed: true }));
  assertFinished(h, 'finished · agreement on 0 at round 1 — nothing can change any more', /round 1\/1 · finished/);
}

{
  const h = await setup();
  await startLive(h);
  h.live.onLiveRound(roundMsg(1, { states: [null, null, null, null], all_undecided: true }));
  assertFinished(h, '⚠ finished · every node ⊥ at round 1 — nothing can recover', /round 1\/1 · all ⊥/);
}

{
  const h = await setup();
  await startLive(h);
  h.live.onLiveRound(roundMsg(1));
  h.$('svEndBtn').click();
  assertFinished(h, 'ended · stopped at round 1/200 by End', /round 1\/1 · ended/);
}

const trace = rounds => ({ n: 4, seed: 1, initial: [0, 0, 1, 1], rounds: Array.from({ length: rounds }, () => ({ states: [0, 0, 0, 0], targets: [], blocked: [] })) });
for (const [outcome, text, label] of [
  [{ Agreement: { value: 0, rounds: 3, availability: 'Held' } }, 'finished · agreement on 0 at round 3', /round 3\/3 · finished/],
  [{ Agreement: { value: 0, rounds: 2, availability: { Extinct: { at: 3 } } } }, '⚠ finished · agreement on 0 at round 2, then every node ⊥ from round 3', /round 3\/3 · finished/],
  [{ AllUndecided: { rounds: 3 } }, '⚠ finished · every node ⊥ at round 3 — nothing can recover', /round 3\/3 · all ⊥/],
  [{ NoConvergence: { rounds: 3 } }, 'finished · 3/3 rounds — no agreement', /round 3\/3 · finished/],
]) {
  const h = await setup(3);
  h.live.runBatchSpec(h.spec, 'permanent');
  h.live.onBatchDone({ ms: 1, json: JSON.stringify({ report: { outcome }, trace: trace(3) }) });
  assertFinished(h, text, label);
}

console.log('single-value end-state tests passed');
