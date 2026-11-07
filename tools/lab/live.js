// single-value session: Start (live or batch), stepping, replay of a finished run,
// toolbar state, export. A live session grows S.trace one round per step; a
// finished run (batch, opened file, or a stopped live session) keeps the whole
// trace, and moving the cursor moves the node grid with it.
import { S, $ } from './state.js';
import { post } from './engine.js';
import { update } from './render.js';
import { loadTrace } from './trace.js';
import { buildSpec, validateSpec, strategy, readProgram, LIVE_STRATEGIES } from './form.js';
import { programBeta } from './smr-derive.js';
import { useSeek, resetSvViewport } from './sv-viewport.js';
import { downloadJson } from './form.js';

const sv = S.sv;
const MAX_REPLAY_STEP = 10;
const total = () => (S.trace ? S.trace.rounds.length : 0);
const running = () => !!S.live;

// Why a finished run stops where it does: { kind, round, max, value, detail }.
function batchEnd(outcome, max) {
  if (outcome.Agreement) {
    const { value, rounds, availability } = outcome.Agreement;
    const detail = availability.Extinct ? `every node ⊥ from round ${availability.Extinct.at}`
      : availability.Collapsing ? `fewer than a third hold a value from round ${availability.Collapsing.at}` : null;
    return { kind: 'agreement', round: rounds, max, value, detail };
  }
  if (outcome.AllUndecided) return { kind: 'bot', round: outcome.AllUndecided.rounds, max };
  return { kind: 'max', round: outcome.NoConvergence.rounds, max, detail: 'no agreement' };
}
function endText(end) {
  switch (end.kind) {
    case 'max': return `finished · ${end.round}/${end.max} rounds` + (end.detail ? ' — ' + end.detail : '');
    case 'agreement': return end.detail
      ? `⚠ finished · agreement on ${end.value} at round ${end.round}, then ${end.detail}`
      : `finished · agreement on ${end.value} at round ${end.round}` + (end.absorbed ? ' — nothing can change any more' : '');
    case 'bot': return `⚠ finished · every node ⊥ at round ${end.round} — nothing can recover`;
    case 'ended': return `ended · stopped at round ${end.round}/${end.max} by End`;
    case 'file': return `opened trace · ${end.round} rounds`;
  }
  throw new Error('unknown end kind ' + end.kind);
}
const END_LABEL = { max: 'finished', agreement: 'finished', bot: 'all ⊥', ended: 'ended', file: 'trace' };

function sessionCtx(spec, kind, sticky, program, st) {
  return { spec, kind, sticky, program, strategy: st, n: spec.n, maxRounds: spec.max_rounds };
}

function configLine(ctx) {
  const s = ctx.spec;
  const blocking = ctx.program ? `scheduled program, ${ctx.program.length} segment${ctx.program.length === 1 ? '' : 's'}`
    : ctx.strategy === 'sticky' ? `sticky (slider + ${Math.round(s.schedule.fraction * 100)}% background)`
    : ctx.strategy === 'fresh' ? `fresh ${Math.round(s.schedule.fraction * 100)}%${ctx.kind === 'live' ? ' (live slider)' : ''}`
    : ctx.strategy === 'permanent' ? `permanent ${Math.round(s.schedule.fraction * 100)}%`
    : ctx.strategy === 'adaptive0' ? `adaptive 0-late ${Math.round(s.schedule.fraction * 100)}% (outside the paper’s model)`
    : ctx.strategy === 'adaptiveAlpha' ? `adaptive ${s.schedule.alpha}-late ${Math.round(s.schedule.fraction * 100)}%`
    : 'scheduled';
  return [`${s.n} nodes`, `(${s.k},${s.ell})-median`, `blocking ${blocking}`, `seed ${s.seed}`].join(' · ');
}
function configRows(ctx) {
  const s = ctx.spec;
  return [['nodes', s.n], ['seed', s.seed], ['k, ℓ', `${s.k}, ${s.ell}`], ['initial values', s.init.kind === 'with_undecided' ? `${s.init.inner.kind} (useful ${s.init.useful_fraction})` : s.init.kind], ['max rounds', s.max_rounds], ['strategy', ctx.strategy]];
}

function showSession(ctx, line, rows) {
  sv.ctx = ctx;
  $('svSumChip').textContent = line + ' ▾';
  $('svSumChip').classList.remove('hidden');
  $('svCfgPop').innerHTML = rows.map(([k, v]) => `<div class="cfgRow"><span class="k">${k}</span><span class="v mono">${v}</span></div>`).join('') +
    (ctx.spec ? '<button class="btn cfgEdit" id="svCfgEdit">edit…</button>' : '');
  $('svCfgPop').classList.remove('hidden');
  const edit = $('svCfgEdit');
  if (edit) edit.addEventListener('click', () => {
    $('svCfgPop').classList.remove('open');
    $('svFormCard').classList.remove('hidden');
    $('svFormCard').classList.add('overlayMode');
  });
  $('svFormCard').classList.add('hidden');
  $('svFormCard').classList.remove('overlayMode');
  $('svRun').classList.remove('hidden');
  S.sel = null;
  resetSvViewport();
  updateToolbar();
}
$('svSumChip').addEventListener('click', e => {
  e.stopPropagation();
  $('svCfgPop').classList.toggle('open');
});
document.addEventListener('click', e => {
  if (!e.target.closest('.cfgWrap')) $('svCfgPop').classList.remove('open');
});

/* ---------- starting ---------- */

$('svStartBtn').addEventListener('click', () => {
  if (S.engine !== 'wasm') { $('runStatus').textContent = 'engine not ready'; return; }
  const spec = buildSpec();
  const problem = validateSpec(spec);
  if (problem) { $('runStatus').textContent = 'error: ' + problem; return; }
  const st = strategy();
  stopReplay();
  if (LIVE_STRATEGIES.includes(st)) {
    const sticky = st === 'sticky';
    const program = st === 'program' ? readProgram() : null;
    sv.pending = sessionCtx(spec, 'live', sticky, program, st);
    S.pendingLiveBg = sticky && spec.schedule.kind === 'fresh_per_round' ? spec.schedule.fraction : 0;
    $('runStatus').textContent = 'starting live session…';
    post({ type: 'live-new', spec, sticky });
  } else {
    runBatchSpec(spec, st);
  }
});

// A spec the live path cannot draw exactly (or any imported spec) runs through
// the batch engine and opens as a finished run.
export function runBatchSpec(spec, st = null) {
  if (S.engine !== 'wasm') { $('runStatus').textContent = 'engine not ready'; return; }
  stopReplay();
  const kind = { permanent: 'permanent', adaptive_0late: 'adaptive0', adaptive_alpha_late: 'adaptiveAlpha' }[spec.schedule?.kind] ?? 'fresh';
  sv.pending = sessionCtx(spec, 'batch', false, null, st ?? kind);
  $('runStatus').textContent = 'running…';
  post({ type: 'run', spec });
}

export function onBatchDone(m) {
  const out = JSON.parse(m.json);
  S.report = out.report;
  S.live = null;
  sv.handle = false;
  loadTrace(out.trace);
  S.t = total();
  const ctx = sv.pending;
  showSession(ctx, configLine(ctx), configRows(ctx));
  setEnd(batchEnd(out.report.outcome, ctx.maxRounds));
  update();
}

export function onLiveReady(m) {
  stopReplay();
  loadTrace(JSON.parse(m.json));
  S.live = { playing: false, awaiting: false, bg: S.pendingLiveBg || 0 };
  sv.handle = false;
  sv.end = null;
  const ctx = sv.pending;
  sv.program = ctx.program;
  const initial = ctx.program ? programBeta(ctx.program, 1) ?? 0 : ctx.sticky ? 0 : Math.round(ctx.spec.schedule.fraction * 100);
  setSlider(initial);
  showSession(ctx, configLine(ctx), configRows(ctx));
  $('runStatus').textContent = 'live session — move the blocking slider to change the stimulus';
  update();
}

// An opened trace file is a finished run with no spec behind it.
export function openTrace(tr) {
  stopReplay();
  S.live = null;
  sv.handle = false;
  loadTrace(tr);
  S.t = total();
  const ctx = { spec: null, kind: 'file', sticky: false, program: null, strategy: 'file', n: tr.n, maxRounds: total() };
  showSession(ctx, `${tr.n} nodes · seed ${tr.seed} · ${total()} rounds (opened trace)`,
    [['nodes', tr.n], ['seed', tr.seed], ['rounds', total()]]);
  setEnd({ kind: 'file', round: total(), max: total() });
  update();
}

/* ---------- live stepping ---------- */

function setSlider(percent) {
  $('liveSlider').value = percent;
  $('liveSliderVal').textContent = $('liveSlider').value + '%';
}
function fractionFor(round) {
  const programmed = programBeta(sv.program, round);
  if (programmed !== null) setSlider(programmed);
  return (+$('liveSlider').value) / 100;
}
const speedDelay = () => (sv.speed === 'max' ? 0 : 700 / sv.speed);

function liveStep() {
  if (!S.live || S.live.awaiting) return;
  if (total() >= sv.ctx.maxRounds) { finishLive({ kind: 'max', round: total(), max: sv.ctx.maxRounds }); return; }
  S.live.awaiting = true;
  post({ type: 'live-step', fraction: fractionFor(total() + 1) });
}

export function onLiveRound(r) {
  if (!S.live) return;
  S.live.awaiting = false;
  if (r.absorbed) { finishLive({ kind: 'agreement', round: r.round, max: sv.ctx.maxRounds, value: r.agreed_value, absorbed: true }); return; }
  const follow = S.t === total();
  S.trace.rounds.push({ states: r.states, targets: r.targets, blocked: r.blocked });
  appendComp(r.states, r.blocked.length);
  if (follow) S.t = total();
  $('svRoundLbl').textContent = 'round ' + r.round;
  const holdAgreed = r.agreed_value !== null && r.all_hold;
  const sliderZero = +$('liveSlider').value === 0;
  update();
  if (r.all_undecided) { finishLive({ kind: 'bot', round: r.round, max: sv.ctx.maxRounds }); return; }
  if (total() >= sv.ctx.maxRounds) { finishLive({ kind: 'max', round: total(), max: sv.ctx.maxRounds }); return; }
  // the engine reports absorption one step later, and only if blocking stays 0
  if (holdAgreed && sliderZero && S.live.bg === 0) {
    S.live.playing = false;
    $('runStatus').textContent = `agreement on ${r.agreed_value} at round ${r.round} · paused — nothing changes at 0% blocking; Play ends the run, raising blocking stresses it`;
  }
  updateToolbar();
  if (S.live.playing) setTimeout(liveStep, speedDelay());
}

function appendComp(states, blockedLen) {
  const { values, vIndex, comp } = S.series;
  const counts = new Array(values.length).fill(0);
  let bot = 0;
  for (const v of states) {
    if (v === null) bot++;
    else {
      if (!(v in vIndex)) { vIndex[v] = values.length; values.push(v); counts.push(0); for (const c of comp) c.counts.push(0); }
      counts[vIndex[v]]++;
    }
  }
  comp.push({ counts, bot, blocked: blockedLen });
}

// The single place a session becomes a finished run: everything that tells the
// user why nothing more can run reads sv.end.
function setEnd(end) {
  sv.end = end;
  $('runStatus').textContent = endText(end);
  updateToolbar();
}
// A stopped live session keeps its engine handle in the worker so spec export
// still works on the frozen run.
function finishLive(end) {
  if (!S.live) return;
  S.live = null;
  sv.handle = true;
  setEnd(end);
  update();
}
function remindEnd() {
  $('runStatus').textContent = endText(sv.end) + ' · Replay plays it again; New run starts another';
  $('runStatus').classList.add('flash');
  setTimeout(() => $('runStatus').classList.remove('flash'), 700);
}

/* ---------- replay of a finished run ---------- */

let replayTimer = null;
function stopReplay() {
  S.playing = false;
  if (replayTimer) { clearInterval(replayTimer); replayTimer = null; }
}
function startReplay() {
  if (S.t >= total()) S.t = 0;
  S.playing = true;
  update();
  replayTimer = setInterval(() => {
    const stepBy = sv.speed === 'max' ? MAX_REPLAY_STEP : 1;
    S.t = Math.min(S.t + stepBy, total());
    if (S.t >= total()) stopReplay();
    update();
    updateToolbar();
  }, sv.speed === 'max' ? 60 : 700 / sv.speed);
  updateToolbar();
}

useSeek(t => seek(t));
export function seek(t) {
  S.t = Math.min(Math.max(t, 0), total());
  if (!running()) stopReplay();
  update();
  updateToolbar();
}

/* ---------- toolbar ---------- */

export function updateToolbar() {
  const lv = S.live;
  const has = !!sv.ctx;
  $('svCtl').classList.toggle('hidden', !has);
  const playing = lv ? lv.playing : S.playing;
  const end = lv ? null : sv.end;
  const atEnd = !!end && S.t === total();
  $('svPlayBtn').textContent = playing ? '⏸ Pause' : end ? '⏵ Replay' : '⏵ Play';
  $('svStepBtn').disabled = atEnd;
  if (atEnd) $('svStepBtn').dataset.tip = `${endText(end)}. Replay plays it again; New run starts another.`;
  else $('svStepBtn').dataset.tip = 'advance one round';
  for (const id of ['liveSlider']) $(id).disabled = !lv;
  $('svStimulus').classList.toggle('off', !lv);
  $('svBackBtn').disabled = !!lv;
  $('svEndBtn').classList.toggle('hidden', !lv);
  $('svNewBtn').classList.toggle('hidden', !!lv || !has);
  for (const b of document.querySelectorAll('#svSpeed button')) b.classList.toggle('on', b.dataset.speed === String(sv.speed));
  $('svStrat').textContent = has ? (sv.ctx.program ? 'program' : sv.ctx.strategy) : '';
  $('svRoundLbl').textContent = end ? `round ${S.t}/${total()} · ${END_LABEL[end.kind]}`
    : 'round ' + S.t + (S.t === total() ? '' : ' / ' + total());
  $('svRoundLbl').classList.toggle('ended', !!end);
  $('svSumChip').classList.toggle('hidden', !has);
}

export function togglePlay() {
  if (!sv.ctx) return;
  if (S.live) {
    S.live.playing = !S.live.playing;
    updateToolbar();
    if (S.live.playing) liveStep();
  } else if (S.playing) { stopReplay(); updateToolbar(); }
  else startReplay();
}
export function stepForward() {
  if (!sv.ctx) return;
  if (S.live) { S.live.playing = false; if (S.t === total()) liveStep(); else seek(S.t + 1); updateToolbar(); }
  else if (S.t === total() && sv.end) remindEnd();
  else seek(S.t + 1);
}
export function stepBack() { if (sv.ctx && !S.live) seek(S.t - 1); }

$('svPlayBtn').addEventListener('click', togglePlay);
$('svStepBtn').addEventListener('click', stepForward);
$('svBackBtn').addEventListener('click', stepBack);
$('svToStart').addEventListener('click', () => seek(0));
$('svToEnd').addEventListener('click', () => seek(total()));
for (const b of document.querySelectorAll('#svSpeed button')) {
  b.addEventListener('click', () => {
    sv.speed = b.dataset.speed === 'max' ? 'max' : +b.dataset.speed;
    S.speed = typeof sv.speed === 'number' ? sv.speed : 1;
    if (S.playing) { stopReplay(); startReplay(); }
    updateToolbar();
  });
}
$('liveSlider').addEventListener('input', () => {
  $('liveSliderVal').textContent = $('liveSlider').value + '%';
  sv.program = null;
});
$('svEndBtn').addEventListener('click', () => finishLive({ kind: 'ended', round: total(), max: sv.ctx.maxRounds }));
$('svNewBtn').addEventListener('click', () => {
  stopReplay();
  S.live = null;
  sv.handle = false;
  sv.ctx = null;
  sv.end = null;
  S.trace = null; S.series = null; S.t = 0; S.sel = null;
  $('svSumChip').classList.add('hidden');
  $('svCfgPop').classList.add('hidden');
  $('svFormCard').classList.remove('hidden', 'overlayMode');
  $('svRun').classList.add('hidden');
  $('svCtl').classList.add('hidden');
  $('runStatus').textContent = '';
});

$('svNetToggle').addEventListener('click', () => {
  sv.srvOpen = !sv.srvOpen;
  $('svNetBody').classList.toggle('hidden', !sv.srvOpen);
  $('svNetToggle').textContent = sv.srvOpen ? '▾' : '▸';
  update();
});
$('edgeMode').addEventListener('change', e => { S.edgeMode = e.target.value; update(); });

/* ---------- export ---------- */

export function onLiveScenario(m) { downloadJson('live-scenario.json', m.json); }
export function onLiveTraceJson(m) { downloadJson('live-trace.json', m.json); }
$('svExportSpec').addEventListener('click', () => {
  if (!sv.ctx) return;
  if (S.live || sv.handle) post({ type: 'live-export' });
  else if (sv.ctx.spec) downloadJson('scenario.json', JSON.stringify(sv.ctx.spec, null, 2));
  else $('runStatus').textContent = 'an opened trace carries no scenario spec';
});
$('svExportTrace').addEventListener('click', () => {
  if (!S.trace) return;
  if (S.live || sv.handle) post({ type: 'live-trace' });
  else downloadJson('trace.json', JSON.stringify(S.trace));
});
