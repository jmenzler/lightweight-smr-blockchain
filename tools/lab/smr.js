// SMR view: run form + injections editor, batch render, live loop with
// interactive injection, audit-drawer command list, spread/position charts,
// prefix ribbon. Shell: A2 (spec 2026-07-23-lab-gui-revamp-design.md).
import { S, $, PALETTE } from './state.js';
import { post } from './engine.js';
import { downloadJson } from './form.js';
import { createPmfEditor } from './pmf-editor.js';
import { onCertsSnapshot, onChainSnapshot, freezeChain, useEntrySelect, resetCertsDrawer, onVerifyTallies, markCertsLoading, endCertsLoading, cmdIndexFor } from './smr-certs.js';
import { applyReportDelta, EMPTY_REPORT } from './smr-delta.js';
import { recRows, rStarScan, releaseRound, lawText, programBeta, smrEnd, endText, END_LABEL, sideTabAfter, spreadingHolders, cmdName, clientModelSpec, poolInFlight, isPool } from './smr-derive.js';
import { renderViewport, resetViewport } from './smr-viewport.js';
import { ROUND_CAP, continuePlan, targetEnd } from './continue.js';
import { bindContinueMenu, continueStatus } from './continue-ui.js';
import { renderDrawer, resetDrawer, blockDrawerOpen, useBlockDrawerOpen, useSelectCmd } from './smr-drawer.js';

S.smr = { report: null, live: null, scenario: null, ctx: null, pending: null, nextClient: 1, nextOp: 1, lastStatus: null, sel: null, proto: 'recovery', speed: 1, handle: false, srvOpen: false, chart: 'spread', program: null, detail: null, pin: null, manualBlocks: new Set(), gridMode: null, gridSort: false, chain: null, ack: true, end: null, ages: null };

/* ---------- arrival traffic (engine pmf) ---------- */

const ARR = createPmfEditor('arr', 'rowArr');
ARR.analytic = 'zipf';
// The form is laid out hidden until the SMR tab opens; redraw once it has a size.
new ResizeObserver(() => ARR.draw()).observe($('arrCanvas'));

// fixed rate r = point mass at index r; fractional = two-point mixture
function rateToPmf(rate) {
  const r = +rate;
  if (Number.isInteger(r)) {
    const w = new Array(r + 1).fill(0);
    w[r] = 1;
    return w;
  }
  return [1 - (r - Math.floor(r)), r - Math.floor(r)];
}

function formTrafficSpec() {
  const v = $('smrArrRate').value;
  if (v === '') return null;
  return { kind: 'pmf', arrivals_pmf: v === 'custom' ? ARR.weights() : rateToPmf(v) };
}

$('smrArrRate').addEventListener('change', () => {
  $('rowArr').classList.toggle('hidden', $('smrArrRate').value !== 'custom');
  if ($('smrArrRate').value === 'custom') ARR.draw();
});

/* ---------- shell: audit drawer, chart tabs, config line ---------- */

$('auditClosedTag').addEventListener('click', () => setAuditOpen(true));
$('auditCollapse').addEventListener('click', () => setAuditOpen(false));
// Sidebar tabs: Summary until a command is selected, Audit for every new selection.
let side = { tab: 'summary', explicit: false };
function renderSide() {
  for (const b of document.querySelectorAll('#sideTabs .ct')) b.classList.toggle('on', b.dataset.side === side.tab);
  $('sideSummary').classList.toggle('hidden', side.tab !== 'summary');
  $('sideAudit').classList.toggle('hidden', side.tab !== 'audit');
  $('sideTagLabel').textContent = side.tab === 'summary' ? 'SUMMARY' : 'AUDIT';
  $('auditCnt').classList.toggle('hidden', side.tab !== 'audit');
}
function sideEvent(event) {
  side = sideTabAfter(side, event);
  renderSide();
}
for (const b of document.querySelectorAll('#sideTabs .ct')) b.addEventListener('click', () => sideEvent({ pick: b.dataset.side }));
function setAuditOpen(open) {
  $('smrAuditCol').classList.toggle('open', open);
  $('smrAuditCol').classList.toggle('closed', !open);
  $('auditClosedTag').classList.toggle('hidden', open);
  $('auditOpenWrap').classList.toggle('hidden', !open);
}

// Everything the page shows about a session is read from this captured
// context, never from the editable form, which may change mid-session.
function sessionCtx(spec, kind, sticky, program = null) {
  const p = spec.proto;
  return {
    kind, spec, sticky, program, n: spec.n, maxRounds: spec.max_rounds, proto: p.kind,
    tw: p.kind === 'recovery' ? p.t_window_rounds : null,
    T: p.kind === 'recovery' ? p.t_window_rounds : p.kind === 'compact' ? p.t_commit_rounds : null,
    certsRec: p.kind === 'recovery' && !!p.resend_until_acked,
  };
}

function blockingText(spec, sticky, kind, program) {
  const s = spec.schedule;
  if (program) return `scheduled program, ${program.length} segment${program.length === 1 ? '' : 's'}`;
  if (s?.fractions) return `scheduled program, ${s.fractions.length} rounds`;
  if (kind === 'live') return sticky ? `sticky (slider + ${Math.round(s.fraction * 100)}% background)` : `fresh ${Math.round(s.fraction * 100)}% (live slider)`;
  if (s?.kind === 'fresh_per_round') return `fresh ${Math.round(s.fraction * 100)}%`;
  return 'none';
}
function trafficText(traffic) {
  if (!traffic) return 'off';
  const pmf = traffic.arrivals_pmf;
  if (!pmf) return `${traffic.phases.length} phases`;
  const total = pmf.reduce((a, w) => a + w, 0);
  return `≈ ${(pmf.reduce((a, w, i) => a + i * w, 0) / total).toFixed(2)} commands / round`;
}
function configLine(ctx) {
  const { spec } = ctx;
  return [`${spec.n} servers`, ctx.proto, ctx.T ? `T ${ctx.T}` : null, `blocking ${blockingText(spec, ctx.sticky, ctx.kind, ctx.program)}`, `seed ${spec.seed}`]
    .filter(Boolean).join(' · ');
}
function configRows(ctx) {
  const { spec } = ctx;
  return [
    ['protocol', ctx.proto + (ctx.T ? `, T ${ctx.T}` : '')],
    ctx.proto === 'recovery' ? ['certificates', ctx.certsRec ? 'on (thesis construction)' : 'off'] : null,
    ['servers', spec.n], ['seed', spec.seed], ['σ', spec.sigma], ['max rounds', spec.max_rounds],
    ['blocking', blockingText(spec, ctx.sticky, ctx.kind, ctx.program)],
    ['traffic', trafficText(spec.traffic)],
    ['clients', isPool(spec) ? `pool of ${spec.client_model.clients}` : 'unique'],
    ['injections', spec.injections.length],
  ].filter(Boolean);
}

function sessionStarted(ctx) {
  S.smr.ctx = ctx;
  S.smr.ages = null;
  S.smr.mem = null;
  S.smr.seqRound = null;
  S.smr.safetyRound = null;
  sideEvent({ select: S.smr.sel ?? null });
  S.smr.blockSel = null;
  resetViewport();
  resetDrawer();
  $('smrSumChip').textContent = configLine(ctx) + ' ▾';
  $('smrSumChip').classList.remove('hidden');
  $('smrCfgPop').innerHTML = configRows(ctx).map(([k, v]) =>
    `<div class="cfgRow"><span class="k">${k}</span><span class="v mono">${v}</span></div>`).join('') +
    '<button class="btn cfgEdit" id="smrCfgEdit">edit…</button>';
  $('smrCfgPop').classList.remove('hidden');
  $('smrCfgEdit').addEventListener('click', () => {
    $('smrCfgPop').classList.remove('open');
    $('smrFormCard').classList.remove('hidden');
    $('smrFormCard').classList.add('overlayMode');
  });
  $('smrFormCard').classList.add('hidden');
  $('smrFormCard').classList.remove('overlayMode');
  $('smrRun').classList.remove('hidden');
}
$('smrSumChip').addEventListener('click', e => {
  e.stopPropagation();
  $('smrCfgPop').classList.toggle('open');
});
document.addEventListener('click', e => {
  if (!e.target.closest('.cfgWrap')) $('smrCfgPop').classList.remove('open');
});
function sessionEnded() {
  S.smr.ctx = null;
  $('smrSumChip').classList.add('hidden');
  $('smrCfgPop').classList.add('hidden');
  $('smrFormCard').classList.remove('hidden', 'overlayMode');
  $('smrCtl').classList.add('hidden');
  $('smrRun').classList.add('hidden');
}

/* ---------- injections editor ---------- */

function injRow(round = 2, client = S.smr.nextClient, op = S.smr.nextOp) {
  const row = document.createElement('div');
  row.className = 'injRow';
  row.innerHTML = `
    <input class="fInput injRound" type="number" min="1" value="${round}" title="round" />
    <input class="fInput injClient" type="number" min="0" value="${client}" title="client id" />
    <input class="fInput injOp" type="number" min="1" value="${op}" title="op (command id)" />
    <input class="fInput injTarget" type="number" min="0" placeholder="rnd" title="target node — empty = random per attempt (paper client)" />
    <button class="injDel" title="remove">✕</button>`;
  row.querySelector('.injDel').addEventListener('click', () => row.remove());
  $('smrInjRows').appendChild(row);
  S.smr.nextClient = client + 1;
  S.smr.nextOp = op + 1;
}
$('smrInjAdd').addEventListener('click', () => injRow(+$('smrRound').value || 2));

function readInjections() {
  return [...document.querySelectorAll('#smrInjRows .injRow')].map(row => {
    const inj = {
      round: Math.max(1, Math.round(+row.querySelector('.injRound').value || 1)),
      client: Math.max(0, Math.round(+row.querySelector('.injClient').value || 0)),
      op: Math.max(1, Math.round(+row.querySelector('.injOp').value || 1)),
    };
    const t = row.querySelector('.injTarget').value;
    if (t !== '') inj.target = Math.max(0, Math.round(+t));
    return inj;
  });
}

/* ---------- spec building ---------- */

const protoSpec = () => {
  if (S.smr.proto === 'compact') return { kind: 'compact', t_commit_rounds: Math.max(1, Math.round(+$('smrT').value || 8)) };
  if (S.smr.proto === 'recovery') return { kind: 'recovery', t_window_rounds: Math.max(1, Math.round(+$('smrTw').value || 40)), resend_until_acked: S.smr.ack };
  return { kind: 'extended' };
};

export function buildSmrSpec() {
  const proto = protoSpec();
  const spec = {
    n: Math.min(Math.max(+$('smrN').value || 64, 4), 200000),
    seed: +$('smrSeed').value || 0,
    k: 6,
    ell: 3,
    sigma: +$('smrSigma').value || 1,
    proto,
    injections: readInjections(),
    max_rounds: Math.min(Math.max(+$('smrMax').value || 200, 1), 5000),
    schedule: strategy() === 'program'
      ? { kind: 'per_round_fractions', fractions: expandProgram(readProgram(), Math.min(Math.max(+$('smrMax').value || 200, 1), 5000)) }
      : { kind: 'fresh_per_round', fraction: Math.min(Math.max(+$('smrBeta').value || 0, 0), 60) / 100 },
  };
  const traffic = formTrafficSpec();
  if (traffic) spec.traffic = traffic;
  const clients = clientModelSpec($('smrClientModel').value, $('smrPoolN').value);
  if (clients) spec.client_model = clients;
  return spec;
}

// Unique stays the default: the form's arrival rate alone cannot size a pool safely, and an
// undersized one aborts the run.
function syncClientModel() {
  const ext = S.smr.proto === 'extended';
  const opt = $('smrClientModel').querySelector('option[value="pool"]');
  opt.disabled = ext;
  opt.textContent = ext ? 'pool of N (not in extended)' : 'pool of N';
  if (ext) $('smrClientModel').value = 'unique';
  $('smrGuardPool').classList.toggle('hidden', !ext);
  const pool = $('smrClientModel').value === 'pool';
  $('smrPoolN').classList.toggle('hidden', !pool);
  const traffic = formTrafficSpec();
  const T = S.smr.proto === 'recovery' ? +$('smrTw').value : +$('smrT').value;
  const flight = Math.round(poolInFlight(traffic?.arrivals_pmf ?? null, S.smr.proto, T));
  const n = clientModelSpec('pool', $('smrPoolN').value).clients;
  $('smrPoolHint').textContent = !pool ? ''
    : !traffic ? 'traffic off: the pool serves no arrivals'
      : `≈ ${flight} commands in flight on average (${S.smr.proto === 'recovery' ? 'rate × 2T' : 'rate × T'}); peaks run to ≈ 2×` +
        (n < 2 * flight ? ` · a pool of ${n} may exhaust and abort the run` : '') +
        ' · a blocking surge stops commits while arrivals continue, so a long enough surge exhausts any pool';
  $('smrPoolHint').classList.toggle('warn', pool && !!traffic && n < 2 * flight);
}
for (const ev of ['input', 'change', 'pointerup']) $('smrFormCard').addEventListener(ev, syncClientModel);

for (const btn of $('smrProtoToggle').children) {
  btn.addEventListener('click', () => {
    S.smr.proto = btn.dataset.proto;
    for (const b of $('smrProtoToggle').children) b.classList.toggle('on', b === btn);
    document.querySelectorAll('.compactOnly').forEach(el =>
      el.classList.toggle('hidden', S.smr.proto !== 'compact'));
    document.querySelectorAll('.recoveryOnly').forEach(el =>
      el.classList.toggle('hidden', S.smr.proto !== 'recovery'));
    syncClientModel();
  });
}
syncClientModel();

for (const btn of $('smrAckToggle').children) {
  btn.addEventListener('click', () => {
    S.smr.ack = btn.dataset.ack === 'on';
    for (const b of $('smrAckToggle').children) b.classList.toggle('on', b === btn);
  });
}

/* ---------- scenario presets (JS-side β programs) ---------- */

// Each preset prefills the form and hands back a β program: piecewise
// [from, to) round ranges driving the slider, so the surge and its release
// happen at exactly the rounds the headline results were measured at.
const PRESETS = {
  custom: null,
  // A full blackout ending strictly INSIDE a window is the regime the
  // release law was measured in: the population is still ⊥ at release, so
  // recovery costs one window to re-arm every server to reset and a second to
  // roll them back. A blackout ending exactly on a boundary gets the re-arm
  // for free and beats the law by a window; a partial surge skips it too.
  surge: { n: 256, tw: 40, sigma: 1, max: 400, beta: 0, rate: '1',
    program: [{ from: 41, to: 111, beta: 100 }] },
  // 30% leaves ~4.2 expected replies — servers drop out without a spiral. Over 3n/4
  // stay unblocked, so the run is benign and reads no release.
  partial: { n: 256, tw: 40, sigma: 1, max: 400, beta: 0, rate: '1',
    program: [{ from: 41, to: 141, beta: 30 }] },
  // T = 2 sits under the broadcast-time floor, so a sustained-but-survivable
  // β eventually hands a checkpoint a P its log no longer extends — the
  // engine refuses to continue and the run ends as an abort.
  cliff: { n: 128, tw: 2, sigma: 1, max: 200, beta: 0, rate: '1',
    program: [{ from: 5, to: 9999, beta: 30 }] },
};

function applyPreset(name) {
  const p = PRESETS[name];
  if (!p) return;
  $('smrStrategy').value = 'program';
  $('smrProgRows').innerHTML = '';
  for (const seg of p.program) progRow(seg.from, seg.to, seg.beta);
  syncStrategy();
  $('smrN').value = p.n;
  $('smrTw').value = p.tw;
  $('smrSigma').value = p.sigma;
  $('smrMax').value = p.max;
  $('smrBeta').value = p.beta;
  // Load is not decoration here: an idle system has no backlog to re-age and
  // no batches to drain, so the queue and blocks panels stay blank without it.
  $('smrArrRate').value = p.rate;
  $('smrArrRate').dispatchEvent(new Event('change'));
}
for (const btn of $('smrPresetToggle').children) {
  btn.addEventListener('click', () => {
    for (const b of $('smrPresetToggle').children) b.classList.toggle('on', b === btn);
    applyPreset(btn.dataset.preset);
  });
}

// The program owns the slider only until the user grabs it.
function clearProgram() {
  S.smr.program = null;
  for (const b of $('smrPresetToggle').children) b.classList.toggle('on', b.dataset.preset === 'custom');
}

/* ---------- blocking strategy ---------- */

// Strategies the SMR engine cannot run live stay listed but disabled, each
// with the reason: a lab run must be the exact run the engine gives for the
// same spec and seed, so nothing is emulated in JS.
const STRATEGY_HINT = {
  fresh: 'A new random set of servers is blocked every round; the slider changes the fraction live.',
  sticky: 'A persistent targeted set follows the slider; the percentage here is never-stopping background noise under it.',
  program: 'Blocked fraction per round segment, drawn from the same stream as a fresh schedule; grabbing the slider takes manual control from that round.',
};
const strategy = () => $('smrStrategy').value;

function syncStrategy() {
  const st = strategy();
  $('rowBeta').classList.toggle('hidden', st === 'program');
  $('rowProgram').classList.toggle('hidden', st !== 'program');
  $('smrBetaLabel').textContent = st === 'sticky' ? 'background blocked % of n' : 'blocked % of n';
  $('smrStrategyTip').dataset.tip = STRATEGY_HINT[st] ?? '';
}
$('smrStrategy').addEventListener('change', syncStrategy);

function progRow(from = 41, to = 111, pct = 100) {
  const row = document.createElement('div');
  row.className = 'progRow';
  row.innerHTML = `
    <input class="fInput progFrom" type="number" min="1" value="${from}" />
    <input class="fInput progTo" type="number" min="2" value="${to}" />
    <input class="fInput progPct" type="number" min="0" max="100" value="${pct}" />
    <button class="injDel" title="remove">✕</button>`;
  row.querySelector('.injDel').addEventListener('click', () => row.remove());
  $('smrProgRows').appendChild(row);
}
$('smrProgAdd').addEventListener('click', () => progRow());

function readProgram() {
  return [...document.querySelectorAll('#smrProgRows .progRow')].map(row => ({
    from: Math.max(1, Math.round(+row.querySelector('.progFrom').value || 1)),
    to: Math.round(+row.querySelector('.progTo').value || 1),
    beta: Math.min(Math.max(+row.querySelector('.progPct').value || 0, 0), 100),
  })).filter(seg => seg.to > seg.from);
}
const expandProgram = (program, rounds) => Array.from({ length: rounds }, (_, i) => (programBeta(program, i + 1) ?? 0) / 100);

/* ---------- batch + live wiring ---------- */

function resetFailureOutcome() {
  S.smr.scenario = null;
  $('smrAbort').classList.add('hidden');
}

// A spec file runs through the engine's batch path and opens as a finished
// run: an export records its per-round schedule and manual blocks, which a
// live session does not replay, and it keeps the batch failure path.
export function runSpecBatch(spec) {
  if (S.engine !== 'wasm') { $('smrStatus').textContent = 'engine not ready'; return; }
  resetFailureOutcome();
  S.smr.live = null;
  $('smrStatus').textContent = 'running spec…';
  S.smr.pending = sessionCtx(spec, 'batch', false, null);
  post({ type: 'smr-run', spec });
}

$('smrStartBtn').addEventListener('click', () => {
  if (S.engine !== 'wasm') { $('smrStatus').textContent = 'engine not ready'; return; }
  resetFailureOutcome();
  const sticky = strategy() === 'sticky';
  const program = strategy() === 'program' ? readProgram() : null;
  $('smrStatus').textContent = 'starting live session…';
  const spec = buildSmrSpec();
  S.smr.pending = sessionCtx(spec, 'live', sticky, program);
  post({ type: 'smr-live-new', spec, sticky });
});

export function onSmrDone(m) {
  S.smr.resume = null;
  S.smr.report = JSON.parse(m.json).report;
  S.smr.scenario = null;
  S.smr.lastStatus = null;
  S.smr.handle = false;
  sessionStarted(S.smr.pending);
  setEnd(smrEnd(S.smr.report.terminal, S.smr.ctx.maxRounds));
  renderSmr();
}

function violationText(v) {
  if ('previous_len' in v) return `executed length fell from ${v.previous_len} to ${v.len}`;
  if ('committed_sn' in v) return `execute-once refused: client ${v.client} committed sn ${v.sn} while its committed sn was ${v.committed_sn}`;
  return `prefix ${v.pre_len}, log ${v.log_len}, first mismatch ${v.first_mismatch}`;
}

export function onSmrFailed(m) {
  const report = JSON.parse(m.report);
  const scenario = JSON.parse(m.scenario);
  const terminal = report.terminal.Failed;
  if (S.smr.live) {
    S.smr.live.playing = false;
    S.smr.live.awaiting = false;
    S.smr.live.runUntil = null;
  }
  S.smr.live = null;
  S.smr.handle = m.mode === 'live';
  S.smr.report = report;
  S.smr.scenario = scenario;
  S.smr.lastStatus = m.status ? JSON.parse(m.status) : null;
  S.smr.detail = null;
  S.smr.pin = null;
  S.smr.chain = null;
  S.smr.manualBlocks = new Set();
  // a live session keeps the ages it streamed up to the failing round
  const ages = m.mode === 'live' && S.smr.ages ? S.smr.ages.concat(m.ages ?? []) : null;
  resetCertsDrawer();
  sessionStarted(sessionCtx(scenario, m.mode, S.smr.pending?.sticky ?? false));
  S.smr.ages = ages;
  setEnd(smrEnd(report.terminal, S.smr.pending?.maxRounds ?? scenario.max_rounds));
  const headline = m.failure.phase === 'BoundaryPreflight'
    ? `recovery stopped before mutating boundary ${terminal.attempted_round}.` +
      `<span class="abortWhy">Completed through r${terminal.completed_round}; valid pre-boundary observations include r${terminal.observed_round}.</span>`
    : `the run stopped in round ${terminal.attempted_round}.` +
      `<span class="abortWhy">Completed through r${terminal.completed_round}; observations include r${terminal.observed_round}.</span>`;
  $('smrAbort').innerHTML =
    `<b>FAILED</b> — ${headline}` +
    `<span class="abortWhy">Node ${m.failure.node}: ${violationText(m.failure.violation)}. The report and replay are frozen and exportable.</span>`;
  $('smrAbort').classList.remove('hidden');
  renderSmr();
}

export function onSmrReady(m) {
  $('smrAbort').classList.add('hidden');
  $('smrSlider').max = S.smr.pending.proto === 'recovery' ? 100 : 60;
  S.smr.handle = false;
  S.smr.detail = null;
  S.smr.pin = null;
  S.smr.manualBlocks = new Set();
  try {
    S.smr.report = applyReportDelta(EMPTY_REPORT, JSON.parse(m.delta));
  } catch (e) {
    S.smr.report = null;
    onSmrAbort({ error: String(e), page: true });
    return;
  }
  S.smr.live = { playing: false, awaiting: false, runUntil: null };
  S.smr.resume = null;
  S.smr.lastStatus = null;
  S.smr.end = null;
  const ctx = S.smr.pending;
  S.smr.program = ctx.program;
  setSlider(ctx.program ? programBeta(ctx.program, 1) ?? 0 : ctx.sticky ? 0 : Math.round((ctx.spec.schedule.fraction ?? 0) * 100));
  S.smr.sel = null;
  sessionStarted(S.smr.pending);
  S.smr.ages = m.ages;
  S.smr.mem = m.memory ? [m.memory] : [];
  $('smrStatus').textContent = 'live session — blocking slider, inject commands anytime';
  updateToolbar();
  // a previous session's rule may have fed the other chain feed
  resetCertsDrawer();
  S.smr.chain = null;
  renderSmr();
}

// The forest panel lives in the block drawer, so the certificate snapshot is read only while
// that drawer is open, and once more when it opens. A recovery session reads the chain delta
// every step, a few new entries at most: the block tiles' margins need entry positions. Merge
// animations after skipped rounds replay just the newest step's merges.
function feedFlags() {
  const open = blockDrawerOpen();
  const certsFeed = !isRec() || displayCertsRec();
  return { wantCerts: open && certsFeed, wantChain: isRec() };
}
useBlockDrawerOpen(() => {
  if (!S.smr.live && !S.smr.handle) return;
  markCertsLoading();
  post({ type: 'smr-live-feeds', ...feedFlags() });
});
export function onSmrFeeds(m) {
  onChainOrCerts(m);
  endCertsLoading();
  renderDrawer();
}

// The chain panel has three feeds: compact streams the §5 certificate
// snapshot whole, recovery-with-certificates streams the same shape over the
// checkpoint forests (RQ11), and plain recovery draws the committed-entry
// deltas that every recovery session accumulates here.
function onChainOrCerts(m) {
  // the round the committed sequence the summary cards read was taken at
  if (S.smr.report && (isRec() ? m.chain && !m.chain.error : m.certs)) S.smr.seqRound = S.smr.report.metrics.length;
  if (!isRec()) { onCertsSnapshot(m.certs); return; }
  const chainError = accumulateChain(m.chain);
  if (displayCertsRec()) { onCertsSnapshot(m.certs, true); return; }
  if (chainError) { freezeChain(chainError); return; }
  if (S.smr.chain && blockDrawerOpen()) onChainSnapshot(S.smr.chain);
}
function accumulateChain(delta) {
  if (!delta) return null;
  if (delta.error) return delta.error;
  const prev = S.smr.chain;
  S.smr.chain = {
    m: delta.m,
    peaks: delta.peaks,
    merges: delta.merges,
    entries: prev ? prev.entries.concat(delta.entries) : delta.entries,
  };
  return null;
}

// A committed block names a command; pinning it is the same act as clicking
// its button in the batch drawer.
useEntrySelect(ent => {
  const rep = S.smr.report;
  if (!rep || ent.kind !== 'cmd') return;
  const i = cmdIndexFor(ent);
  if (i >= 0) selectCmd(i);
});
useSelectCmd(i => selectCmd(i));

// While `S.smr.live` is set the session can step. A stopped live session
// (End, max rounds, dead, failed) keeps its engine handle in the worker so its
// export, verify and node-detail calls keep working on the frozen run.
const MAX_CHUNK = 25;
const lastRound = () => (S.smr.lastStatus ? S.smr.lastStatus.round : 0);
const speedDelay = () => (S.smr.speed === 'max' ? 0 : 720 / S.smr.speed);
const speedChunk = () => (S.smr.speed === 'max' ? MAX_CHUNK : 1);
const blockingStrategy = ctx => (ctx.program || ctx.spec.schedule?.fractions ? 'program' : ctx.sticky ? 'sticky' : 'fresh');

function updateToolbar() {
  const lv = S.smr.live;
  const end = S.smr.end;
  const hasSession = !!S.smr.ctx;
  $('smrCtl').classList.toggle('hidden', !hasSession);
  for (const id of ['smrPlayBtn', 'smrStepBtn', 'smrJumpBtn', 'smrInjectPopBtn', 'smrTrafficPopBtn', 'smrSlider']) $(id).disabled = !lv;
  for (const b of document.querySelectorAll('#smrSpeed button')) b.disabled = !lv;
  if (end) $('smrPlayBtn').dataset.tip = `${endText(end)}. Nothing more can run: New run starts another; Export keeps this one.`;
  else delete $('smrPlayBtn').dataset.tip;
  if (hasSession) {
    const round = end ? end.round : lastRound();
    $('smrLiveRound').textContent = (S.smr.ctx.open && !end ? `round ${round} · until stopped` : `round ${round}/${S.smr.ctx.maxRounds}`) + (end ? ' · ' + END_LABEL[end.kind] : '');
    $('smrLiveRound').classList.toggle('ended', !!end);
  }
  $('smrStimulus').classList.toggle('off', !lv);
  $('smrEndBtn').classList.toggle('hidden', !lv);
  $('smrNewBtn').classList.toggle('hidden', !!lv || !hasSession);
  $('smrContinuePop').classList.toggle('hidden', !!lv || !S.smr.resume);
  if (S.smr.resume) refreshContinue();
  $('smrPlayBtn').textContent = lv && lv.playing ? '⏸ Pause' : '⏵ Play';
  for (const b of document.querySelectorAll('#smrSpeed button')) b.classList.toggle('on', b.dataset.speed === String(S.smr.speed));
  for (const b of document.querySelectorAll('.recoveryItem')) b.disabled = !isRec();
  $('smrStrat').textContent = hasSession ? blockingStrategy(S.smr.ctx) : '';
}

// The single place a session becomes a finished run: everything that tells the
// user why nothing more can run reads S.smr.end.
function setEnd(end) {
  S.smr.end = end;
  $('smrStatus').textContent = endText(end);
  updateToolbar();
}
function finishLive(end) {
  const lv = S.smr.live;
  if (!lv) return;
  lv.playing = false;
  lv.runUntil = null;
  S.smr.live = null;
  // Only a reached round target can go on: the engine itself has no round cap.
  S.smr.resume = end.kind === 'max' && S.smr.ctx.maxRounds < ROUND_CAP ? lv : null;
  S.smr.handle = true;
  setEnd(end);
  renderSmr();
}
// a Play or step key on a finished run re-states why, instead of doing nothing
function remindEnd() {
  $('smrStatus').textContent = endText(S.smr.end) + ' · New run starts another';
  $('smrStatus').classList.add('flash');
  setTimeout(() => $('smrStatus').classList.remove('flash'), 700);
}

function scheduleSmrStep(session, delay, count = 1) {
  setTimeout(() => {
    if (S.smr.live === session) smrStep(count);
  }, delay);
}

function setSlider(percent) {
  $('smrSlider').value = Math.min(percent, +$('smrSlider').max);
  $('smrSliderVal').textContent = $('smrSlider').value + '%';
}
// The program owns the slider only until the user grabs it.
function fractionFor(round) {
  const programmed = programBeta(S.smr.program, round);
  if (programmed !== null) setSlider(programmed);
  return (+$('smrSlider').value) / 100;
}

function smrStep(count = 1) {
  const lv = S.smr.live;
  if (!lv || lv.awaiting) return;
  const room = S.smr.ctx.maxRounds - lastRound();
  if (room <= 0) { finishLive(targetEnd(lastRound(), S.smr.ctx.maxRounds)); return; }
  const n = Math.min(count, room);
  const first = lastRound() + 1;
  lv.awaiting = true;
  if (n === 1) { post({ type: 'smr-live-step', fraction: fractionFor(first), ...feedFlags() }); return; }
  post({ type: 'smr-live-steps', fractions: Array.from({ length: n }, (_, i) => fractionFor(first + i)), ...feedFlags() });
}

// arrival traffic is engine-side: the live pmf edit lands as a phase from the
// next round and rides in the exported spec (replays byte-identically)
$('smrTrafficApply').addEventListener('click', () => {
  if (!S.smr.live) { $('smrStatus').textContent = 'start a live session first'; return; }
  post({ type: 'smr-live-traffic', pmf: rateToPmf($('smrTraffic').value) });
  $('smrTrafficPopBtn').textContent = 'Traffic: ' + $('smrTraffic').value + '/r';
});
export function onSmrTrafficSet(m) {
  const out = JSON.parse(m.json);
  $('smrStatus').textContent = out.error ? 'traffic: ' + out.error : 'arrival pmf applied — takes effect next round';
}
export function onSmrRound(m) {
  const lv = S.smr.live;
  if (!lv) return;
  lv.awaiting = false;
  const status = JSON.parse(m.status);
  try {
    S.smr.report = applyReportDelta(S.smr.report, JSON.parse(m.delta));
  } catch (e) {
    onSmrAbort({ error: String(e), page: true });
    return;
  }
  S.smr.lastStatus = status;
  S.smr.ages = S.smr.ages.concat(m.ages);
  if (m.memory) S.smr.mem.push(m.memory);
  if (S.smr.ctx.proto !== 'extended' && S.smr.ages.length !== S.smr.report.metrics.length) {
    onSmrAbort({ error: `unsettled ages cover ${S.smr.ages.length} rounds, the report ${S.smr.report.metrics.length}`, page: true });
    return;
  }
  // a batched step cannot say which round of the batch flipped the flag
  if (!S.smr.report.safety_ok && S.smr.safetyRound == null && m.steps === undefined) S.smr.safetyRound = status.round;
  const max = S.smr.ctx.maxRounds;
  const stopNote = status.absorbed || status.dead !== null
    ? smrEnd({ Dead: { round: status.dead ?? status.round } }, max)
    : status.round >= max ? targetEnd(status.round, max) : null;
  if (stopNote) lv.playing = false;
  updateToolbar();
  onChainOrCerts(m);
  if (S.smr.pin !== null && isRec()) post({ type: 'smr-node-detail', node: S.smr.pin, pinned: true });
  renderSmr();
  if (lv.runUntil) {
    lv.runSteps++;
    const hit = lv.runUntil(status, S.smr.report);
    const done = hit === true || (typeof hit === 'string' && hit);
    if (done || stopNote || lv.runSteps >= 2000) {
      $('smrStatus').textContent = done
        ? `stopped at r${status.round} — ${typeof hit === 'string' ? hit : lv.runLabel}`
        : `stopped at r${status.round} — no ${lv.runLabel} within ${lv.runSteps} rounds`;
      lv.runUntil = null;
    } else {
      scheduleSmrStep(lv, 0);
      return;
    }
  }
  if (stopNote) { finishLive(stopNote); return; }
  if (lv.playing) scheduleSmrStep(lv, speedDelay(), speedChunk());
}
$('smrPlayBtn').addEventListener('click', () => {
  if (!S.smr.live) return;
  S.smr.live.playing = !S.smr.live.playing;
  updateToolbar();
  if (S.smr.live.playing) smrStep(speedChunk());
});
$('smrStepBtn').addEventListener('click', () => {
  if (!S.smr.live) return;
  S.smr.live.playing = false;
  updateToolbar();
  smrStep();
});
// the replay shortcuts carry over: space plays or pauses, the arrows step
window.addEventListener('keydown', e => {
  if (S.view !== 'smr' || ['INPUT', 'SELECT', 'TEXTAREA'].includes(e.target.tagName)) return;
  if (!S.smr.live) {
    if (S.smr.end && (e.key === ' ' || e.key === 'ArrowRight')) { e.preventDefault(); remindEnd(); }
    return;
  }
  if (e.key === ' ') { e.preventDefault(); $('smrPlayBtn').click(); }
  else if (e.key === 'ArrowRight') { e.preventDefault(); $('smrStepBtn').click(); }
});
$('smrSlider').addEventListener('input', () => {
  $('smrSliderVal').textContent = $('smrSlider').value + '%';
  clearProgram();
});
for (const b of document.querySelectorAll('#smrSpeed button')) {
  b.addEventListener('click', () => {
    S.smr.speed = b.dataset.speed === 'max' ? 'max' : +b.dataset.speed;
    updateToolbar();
  });
}
$('smrAuditScope').addEventListener('change', () => {
  if (S.smr.report) renderAudit(S.smr.report);
});
$('smrEndBtn').addEventListener('click', () => finishLive({ kind: 'ended', round: lastRound(), max: S.smr.ctx.maxRounds }));
const refreshContinue = bindContinueMenu('smr', { ctx: () => S.smr.ctx, current: lastRound, go: continueRun });
function continueRun(mode) {
  const lv = S.smr.resume;
  const plan = lv ? continuePlan(lastRound(), mode) : null;
  if (!plan) return;
  S.smr.resume = null;
  S.smr.ctx.maxRounds = plan.target;
  S.smr.ctx.open = plan.open;
  S.smr.end = null;
  S.smr.live = lv;
  lv.playing = true;
  $('smrStatus').textContent = continueStatus(plan);
  updateToolbar();
  smrStep(speedChunk());
}
$('smrNewBtn').addEventListener('click', () => {
  S.smr.resume = null;
  S.smr.handle = false;
  S.smr.end = null;
  S.smr.report = null;
  S.smr.lastStatus = null;
  S.smr.scenario = null;
  S.smr.chain = null;
  S.smr.pin = null;
  S.smr.detail = null;
  resetFailureOutcome();
  resetCertsDrawer();
  sessionEnded();
});

/* ---------- interactive injection ---------- */

$('smrInjectBtn').addEventListener('click', () => {
  if (!S.smr.live) { $('smrStatus').textContent = 'start a live session first'; return; }
  const client = Math.max(0, Math.round(+$('smrInjClient').value || 0));
  const op = Math.max(1, Math.round(+$('smrInjOp').value || 1));
  const t = $('smrInjTarget').value;
  post({ type: 'smr-live-inject', client, op, target: t === '' ? null : Math.max(0, Math.round(+t)), ...feedFlags() });
});
export function onSmrInjected(m) {
  const out = JSON.parse(m.json);
  onChainOrCerts(m);
  if (m.quiet) return;
  if (out.error) { $('smrStatus').textContent = 'inject: ' + out.error; return; }
  $('smrStatus').textContent = `command accepted — lands round ${out.round}`;
  $('smrInjClient').value = +$('smrInjClient').value + 1;
  $('smrInjOp').value = +$('smrInjOp').value + 1;
  if (S.smr.live && !S.smr.live.playing) smrStep();
}

$('certVerifyBtn').addEventListener('click', () => {
  if (!S.smr.live) { $('smrStatus').textContent = 'start a live session first'; return; }
  post({ type: 'smr-verify-all' });
});
export function onSmrCaptured(m) {
  const out = JSON.parse(m.json);
  if (out.error) { $('smrStatus').textContent = 'capture: ' + out.error; return; }
  $('smrStatus').textContent = `stale cert captured — sn ${out.sn} frozen; two more commits from that client evict it`;
  onCertsSnapshot(m.certs);
}
export function onSmrVerify(m) {
  onVerifyTallies(m.json);
}

// A captured failure scenario or a frozen batch spec exports as run; only a
// stepping or stopped live session asks the engine for its recorded spec.
function exportSpec() {
  if (S.smr.scenario) { downloadJson('smr-scenario.json', JSON.stringify(S.smr.scenario, null, 2)); return; }
  if (S.smr.live || S.smr.handle) { post({ type: 'smr-live-export' }); return; }
  const spec = S.smr.ctx ? S.smr.ctx.spec : buildSmrSpec();
  downloadJson('smr-scenario.json', JSON.stringify(spec, null, 2));
}
$('smrExportSpec').addEventListener('click', exportSpec);
$('smrFormExportSpec').addEventListener('click', () => downloadJson('smr-scenario.json', JSON.stringify(buildSmrSpec(), null, 2)));
// A live or stopped session exports the engine's own report, never the page's merge of deltas.
$('smrExportReport').addEventListener('click', () => {
  if (S.smr.live || S.smr.handle) { post({ type: 'smr-live-report' }); return; }
  if (S.smr.report) downloadJson('smr-report.json', JSON.stringify(S.smr.report, null, 2));
});
export function onSmrReport(m) { downloadJson('smr-report.json', JSON.stringify(JSON.parse(m.json), null, 2)); }
export function onSmrScenario(m) { downloadJson('smr-live-scenario.json', m.json); }

/* ---------- abort as an outcome ---------- */

// A violated oracle is a result, not a crash: the session stops but its
// history stays on screen with the engine's own message.
export function onSmrAbort(m) {
  S.smr.live = null;
  S.smr.handle = false;
  if (S.smr.ctx) setEnd({ kind: 'aborted', round: lastRound(), max: S.smr.ctx.maxRounds });
  else updateToolbar();
  $('smrAbort').innerHTML =
    `<b>ABORTED</b> — ${m.page ? 'the page lost sync with the engine.' : 'the engine refused to continue.'}` +
    `<span class="abortWhy">${escapeHtml(m.panic || m.error)}</span>` +
    `<span class="abortWhy">The run up to here is still on screen and exportable. ` +
    `Start a new session to continue.</span>`;
  $('smrAbort').classList.remove('hidden');
}
function escapeHtml(s) {
  return String(s).replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
}

/* ---------- node drilldown + targeted blocking ---------- */

export function onSmrNodeDetail(m) {
  const d = JSON.parse(m.json);
  S.smr.detail = d.error ? null : d;
  renderSmr();
}
export function onSmrBlocked(m) {
  const out = JSON.parse(m.json);
  if (out.error) { $('smrStatus').textContent = 'block: ' + out.error; return; }
  if (m.blocked) S.smr.manualBlocks.add(m.node); else S.smr.manualBlocks.delete(m.node);
  $('smrStatus').textContent = `node ${m.node} ${m.blocked ? 'blocked' : 'released'} from round ${out.round}`;
  renderSmr();
}

/* ---------- recovery clock, phase, law ---------- */

const displayProto = () => S.smr.ctx?.proto ?? S.smr.proto;
const displayTw = () => S.smr.ctx?.tw ?? null;
const displayCertsRec = () => S.smr.ctx?.certsRec ?? false;
const displayN = () => S.smr.ctx?.n ?? 0;
const isRec = () => displayProto() === 'recovery' && !!displayTw();
function renderRecChips(rep) {
  const on = isRec();
  $('smrRecChips').classList.toggle('hidden', !on);
  if (!on) { $('smrLawCard').classList.add('hidden'); return; }
  const T = displayTw();
  const rows = recRows(rep);
  const round = rep.metrics.length;
  if (!rows || !round) return;
  const w = rows[round - 1].window;
  const nextAt = (Math.floor((round - 1) / T) + 1) * T;
  const n = displayN();
  const failed = !!rep.terminal?.Failed;
  const release = releaseRound(rep, n);
  const rStar = failed || release === null ? null : rStarScan(rep, n, T);

  let clock = `W ${w} · next b${nextAt}`;
  if (release !== null && rStar !== null) clock += ` · r* ${rStar} · R ${rStar - release}`;
  $('smrClockChip').textContent = clock;

  // Honest gating: r*/law only exist once a release was observed, and only
  // harden to CONFIRMED once the scan finds a clean converged suffix.
  const phase = failed || release === null ? null : rStar === null ? 'PROVISIONAL' : 'CONFIRMED';
  $('smrPhaseChip').classList.toggle('hidden', phase === null);
  $('smrPhaseChip').textContent = phase === null ? '' : `${phase} · release r${release}`;
  $('smrPhaseChip').className = 'recChip mono' + (phase === 'CONFIRMED' ? ' ok' : phase ? ' warn' : '');

  renderLawCard(release, rStar, T);
}

function renderLawCard(release, rStar, T) {
  const tip = $('smrLawCard');
  if (release === null || rStar === null) { tip.classList.add('hidden'); return; }
  tip.textContent = `R ${rStar - release}`;
  tip.dataset.tip = lawText(release, rStar, T);
  tip.classList.remove('hidden');
}

/* ---------- steppers ---------- */

// One predicate drives the existing single-flight loop; onSmrRound checks it
// after the state update and either stops or queues the next step.
function runUntil(pred, label) {
  const lv = S.smr.live;
  if (!lv) { $('smrStatus').textContent = 'start a live session first'; return; }
  lv.playing = false;
  lv.runUntil = pred;
  lv.runLabel = label;
  lv.runSteps = 0;
  updateToolbar();
  smrStep();
}

// Rows are PRE-boundary: a boundary's effects land on the row after it, so
// that is where the stepper stops.
const boundaryPred = st => st.round > 1 && (st.round - 1) % displayTw() === 0;

function eventsAt(st, rep) {
  const rows = recRows(rep);
  const row = rows && rows[st.round - 1];
  if (!row) return [];
  const prev = st.round > 1 ? rows[st.round - 2] : null;
  const found = [];
  if (row.rollbacks > 0) found.push({ kind: 'rollback', label: `rollback ×${row.rollbacks} (depth ${row.rollback_depth})` });
  if (row.bot_r > 0 && (!prev || prev.bot_r === 0)) found.push({ kind: 'bot', label: 'first ⊥' });
  if (row.cp_fork_k > 1) found.push({ kind: 'fork', label: `cp-fork k=${row.cp_fork_k}` });
  const n = displayN();
  if (releaseRound(rep, n) !== null && rStarScan(rep, n, displayTw()) === st.round) found.push({ kind: 'confirmed', label: 'recovery CONFIRMED' });
  return found;
}
const eventPred = kind => (st, rep) => {
  const hit = eventsAt(st, rep).find(e => kind === 'event' || e.kind === kind);
  return hit ? hit.label : null;
};

for (const item of document.querySelectorAll('#smrJumpPop .menuItem')) {
  item.addEventListener('click', () => {
    $('smrJumpPop').classList.remove('open');
    const kind = item.dataset.jump;
    if (kind === 'boundary') runUntil(boundaryPred, 'boundary');
    else runUntil(eventPred(kind), kind === 'event' ? 'event' : item.textContent);
  });
}

/* ---------- rendering ---------- */

const cmdColor = i => PALETTE[i % PALETTE.length];
const cmdLabel = cmd => cmdName(cmd, isPool(S.smr.ctx?.spec));
const BADGE = { Complete: ['badge-ok', 'Complete'], Pending: ['badge-run', 'Pending'], Dead: ['badge-dead', 'Dead'] };
const OC = {
  Delivered: ['oc-delivered', 'delivered'],
  TargetBlocked: ['oc-blocked', 'target-blocked'],
  TargetBot: ['oc-bot', 'target-⊥'],
  Amplified: ['oc-amplified', 'amplified'],
  AckCommitted: ['oc-ack', 'ack-committed'],
  Ignored: ['oc-ignored', 'ignored'],
};

function renderSmr() {
  const rep = S.smr.report;
  if (!rep) return;
  const rec = isRec();
  renderServers(rep);
  renderViewport(rep);
  renderDrawer();
  renderMetrics(rep);
  renderRecChips(rep);
  renderAudit(rep);
}

/* selected command: user pick, else the latest injected one */
function selectedCmd(rep) {
  if (S.smr.sel !== null && S.smr.sel < rep.commands.length) return S.smr.sel;
  return rep.commands.length ? rep.commands.length - 1 : null;
}
function selectCmd(i) {
  S.smr.sel = i;
  sideEvent({ select: i });
  renderSmr();
  revealSelected();
}

// Scrolls the audit list alone, so a pick made in a drawer does not move the page.
function revealSelected() {
  const box = $('smrAudit');
  const card = box.querySelector('.auditCard.sel');
  if (!card) return;
  const at = card.getBoundingClientRect().top - box.getBoundingClientRect().top;
  if (at < 0 || at + card.offsetHeight > box.clientHeight) box.scrollTop += at - 8;
}

function prepCanvas(cv) {
  const dpr = window.devicePixelRatio || 1;
  const W = cv.clientWidth, H = cv.clientHeight;
  if (W < 2 || H < 2) return null;
  cv.width = W * dpr; cv.height = H * dpr;
  const ctx = cv.getContext('2d');
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, W, H);
  return { ctx, W, H };
}

// Servers section. Counts come from the report's newest row; the strip and the
// grid need the live status, which a spec (batch) run lacks. The grid shows the
// newest round, also on a finished run.
const LAG_NEAR = 5;
// Knowledge counts, per server, how many of the spreading commands it holds,
// from the status' holder lists. Measured with 60 spreading commands the count
// costs 0.1-0.3 ms at n = 1000 and 1.3 ms at n = 8000, but the status message
// grows to 2 MB there, so the mode stops at 2000 until the engine delivers the
// counts itself.
const KNOW_MAX_N = 2000;
const LAG_COLORS = ['#3dd6c4', '#e0c27a', '#b9691f'];
const frontier = st => st.nodes.reduce((a, g) => Math.max(a, g.executed_len), 0);
const lagBucket = lag => (lag === 0 ? 0 : lag <= LAG_NEAR ? 1 : 2);

function knowledgeShares(st) {
  const total = st.spreading.length;
  const held = new Uint16Array(st.nodes.length);
  for (const c of st.spreading) for (const i of c.holders) held[i]++;
  const shares = total ? Array.from(held, x => x / total) : null;
  let min = Infinity, max = -Infinity;
  if (shares) st.nodes.forEach((g, i) => { if (g.log_len !== null) { min = Math.min(min, shares[i]); max = Math.max(max, shares[i]); } });
  return { total, shares, range: { min, max } };
}

// Knowledge is the default wherever it is available; Lag is the fallback.
function gridMode(st, n) {
  const mode = S.smr.gridMode ?? (n <= KNOW_MAX_N ? 'know' : 'lag');
  if (mode === 'rstate' && !isRec()) return 'lag';
  if (mode === 'know' && n > KNOW_MAX_N) return 'lag';
  if (mode === 'watch' && S.smr.sel === null) return 'lag';
  return mode;
}

// Per-server look for the current mode: colour, optional alpha and a sort key.
// ⊥ servers are grey and sort last in every mode.
function gridLook(rep, st, mode) {
  const n = st.nodes.length;
  const f = frontier(st);
  const know = mode === 'know' ? knowledgeShares(st) : null;
  let holders = null;
  const sel = S.smr.sel;
  if (mode === 'watch' && sel !== null) {
    holders = spreadingHolders(rep.commands[sel], st.spreading);
  }
  return st.nodes.map((g, i) => {
    if (g.log_len === null) return { color: '#4b5563', alpha: 1, key: 1e9 };
    if (mode === 'lag') { const lag = f - g.executed_len; return { color: LAG_COLORS[lagBucket(lag)], alpha: 1, key: lag, lag }; }
    if (mode === 'know') {
      if (!know.shares) return { color: '#252c36', alpha: 1, key: 0 };
      // brightness stretches the round's actual min..max share over the whole
      // range; the sort key stays the true share
      const { min, max } = know.range;
      const stretched = max === min ? 1 : (know.shares[i] - min) / (max - min);
      return { color: '#3dd6c4', alpha: 0.15 + 0.85 * stretched, key: -know.shares[i] };
    }
    if (mode === 'rstate') return { color: g.r === 0 ? '#3dd6c4' : g.r === 1 ? '#e0af68' : '#4b5563', alpha: 1, key: g.r };
    const holds = holders === 'all' || (holders && holders.has(i));
    return { color: holds ? cmdColor(sel) : '#252c36', alpha: 1, key: holds ? 0 : 1 };
  });
}

function gridSummary(rep, st, mode, looks) {
  const n = st.nodes.length;
  const bot = st.nodes.filter(g => g.log_len === null).length;
  const blocked = st.blocked.length;
  if (mode === 'lag') {
    const c = [0, 0, 0];
    looks.forEach(l => { if (l.lag !== undefined) c[lagBucket(l.lag)]++; });
    return `lag: ${c[0]} up to date · ${c[1]} ≤ ${LAG_NEAR} · ${c[2]} > ${LAG_NEAR} · ${bot} ⊥ · ${blocked} blocked`;
  }
  if (mode === 'know') {
    const k = knowledgeShares(st);
    return k.total ? `knowledge: ${k.total} command${k.total === 1 ? '' : 's'} spreading · ${bot} ⊥ · ${blocked} blocked` : `knowledge: no command is spreading · ${bot} ⊥ · ${blocked} blocked`;
  }
  if (mode === 'rstate') {
    const c = st.nodes.reduce((a, g) => (a[g.r]++, a), [0, 0, 0]);
    return `no-reset ${c[0]} · reset ${c[1]} · ⊥ ${c[2]} · ${blocked} blocked`;
  }
  const held = looks.filter(l => l.key === 0).length;
  return `${cmdLabel(rep.commands[S.smr.sel])}: held by ${held}/${n} · ${bot} ⊥ · ${blocked} blocked`;
}

const LEGENDS = {
  lag: () => `<span class="g gNote">lights up only when servers fall behind the commit frontier</span><span class="g"><span class="gd" style="background:${LAG_COLORS[0]}"></span>up to date</span><span class="g"><span class="gd" style="background:${LAG_COLORS[1]}"></span>1–${LAG_NEAR} behind</span><span class="g"><span class="gd" style="background:${LAG_COLORS[2]}"></span>&gt; ${LAG_NEAR} behind</span>`,
  know: st => {
    const k = knowledgeShares(st);
    if (!k.shares) return '<span class="g">no command is spreading</span>';
    const pct = x => Math.round(x * 100) + '%';
    return `<span class="g"><span class="gd" style="background:#3dd6c4;opacity:.2"></span>dim: holds ${pct(k.range.min)}</span><span class="g"><span class="gd" style="background:#3dd6c4"></span>bright: holds ${pct(k.range.max)}</span><span class="g">of ${k.total} spreading · brightness stretched to this round’s range</span>`;
  },
  rstate: () => `<span class="g"><span class="gd" style="background:#3dd6c4"></span>no-reset</span><span class="g"><span class="gd" style="background:#e0af68"></span>reset</span>`,
  watch: () => '<span class="g"><span class="gd watch"></span>holds it</span>',
};
const LEGEND_TAIL = '<span class="g"><span class="gd bot"></span>⊥</span><span class="g"><span class="gd blk"></span>blocked</span>';

function renderServers(rep) {
  const st = S.smr.lastStatus;
  const m = rep.metrics[rep.metrics.length - 1];
  const n = displayN();
  const atNote = S.smr.live ? '' : ` · at r${rep.metrics.length}`;
  const mode = st ? gridMode(st, st.nodes.length) : S.smr.gridMode;
  const looks = st ? gridLook(rep, st, mode) : null;
  $('srvCounts').textContent = st
    ? gridSummary(rep, st, mode, looks) + atNote
    : m ? `${m.nonbot_logs} with value · ${n - m.nonbot_logs} ⊥ · ${m.blocked} blocked${atNote}` : '';
  $('srvBody').classList.toggle('hidden', !S.smr.srvOpen);
  $('srvToggle').textContent = S.smr.srvOpen ? '▾' : '▸';
  $('srvHint').classList.toggle('hidden', !S.smr.srvOpen || !isRec() || !st);
  $('srvHint').textContent = 'click a server to pin it';
  $('srvNote').classList.toggle('hidden', !!st);
  $('smrGridTabs').classList.toggle('hidden', !st || !S.smr.srvOpen);
  $('srvSort').classList.toggle('hidden', !st || !S.smr.srvOpen);
  $('srvSort').classList.toggle('on', S.smr.gridSort);
  for (const b of document.querySelectorAll('#smrGridTabs .ct')) b.classList.toggle('on', b.dataset.grid === mode);
  $('srvKnowBtn').disabled = !!st && st.nodes.length > KNOW_MAX_N;
  $('srvKnowBtn').dataset.tip = st && st.nodes.length > KNOW_MAX_N
    ? 'needs per-server counts from the engine' : 'share of the currently spreading commands this server holds';
  $('srvWatchBtn').disabled = S.smr.sel === null;
  const cmd = S.smr.sel !== null ? rep.commands[S.smr.sel] : null;
  $('srvWatch').classList.toggle('hidden', !cmd || !st);
  if (cmd) $('srvWatchLbl').textContent = 'watching ' + cmdLabel(cmd) + ' ';
  for (const b of document.querySelectorAll('#smrGridTabs .recoveryOnly')) b.classList.toggle('hidden', !isRec());
  drawServerStrip(m, n, st, looks);
  if (S.smr.srvOpen && st) drawSmrNet(rep, mode, looks);
}
$('srvToggle').addEventListener('click', () => {
  S.smr.srvOpen = !S.smr.srvOpen;
  if (S.smr.report) renderSmr();
});

// slot order: server id, or sorted by the mode's value with id as tie-break
function slotOrder(looks) {
  const ids = looks.map((_, i) => i);
  if (S.smr.gridSort) ids.sort((a, b) => looks[a].key - looks[b].key || a - b);
  return ids;
}

function drawServerStrip(m, n, st, looks) {
  const P = prepCanvas($('srvStrip'));
  if (!P || !m || !n) return;
  const { ctx, W, H } = P;
  const blocked = new Set(st ? st.blocked : []);
  if (st && n <= 160) {
    const order = slotOrder(looks);
    const r = Math.min(H / 2 - 1, W / n / 2 - 0.5);
    order.forEach((node, slot) => {
      const x = (slot + 0.5) * (W / n), y = H / 2;
      ctx.beginPath(); ctx.arc(x, y, Math.max(r, 1), 0, Math.PI * 2);
      ctx.fillStyle = looks[node].color; ctx.globalAlpha = looks[node].alpha;
      ctx.fill(); ctx.globalAlpha = 1;
      if (blocked.has(node)) { ctx.strokeStyle = '#f7768e'; ctx.lineWidth = 1; ctx.stroke(); }
    });
    return;
  }
  // too many servers for one dot each: a share bar of the mode's colour classes
  if (st) {
    const groups = new Map();
    for (const l of looks) { const k = l.color + l.alpha.toFixed(1); groups.set(k, { l, c: (groups.get(k)?.c || 0) + 1 }); }
    let x = 0;
    for (const { l, c } of [...groups.values()].sort((a, b) => a.l.key - b.l.key)) {
      ctx.fillStyle = l.color; ctx.globalAlpha = l.alpha;
      ctx.fillRect(x, 2, (c / n) * W, H - 4);
      x += (c / n) * W;
    }
    ctx.globalAlpha = 1;
  } else {
    let x = 0;
    for (const [count, col] of [[m.nonbot_logs, '#3dd6c4'], [n - m.nonbot_logs, '#4b5563']]) {
      ctx.fillStyle = col;
      ctx.fillRect(x, 2, (count / n) * W, H - 4);
      x += (count / n) * W;
    }
  }
  ctx.fillStyle = '#f7768e';
  ctx.fillRect(0, H - 3, (m.blocked / n) * W, 2);
}

function drawSmrNet(rep, mode, looks) {
  const st = S.smr.lastStatus;
  const P = prepCanvas($('smrNet'));
  if (!P) return;
  const { ctx, W, H } = P;
  const n = st.nodes.length;
  const cols = Math.max(1, Math.ceil(Math.sqrt(n * W / Math.max(1, H))));
  const rows = Math.ceil(n / cols);
  const cw = W / cols, ch = H / rows;
  const rad = Math.min(7, Math.max(1.5, Math.min(cw, ch) * 0.32));
  const blocked = new Set(st.blocked);
  const order = slotOrder(looks);
  const slotOf = new Array(n);
  order.forEach((node, slot) => { slotOf[node] = slot; });
  const at = i => ({ x: (slotOf[i] % cols) * cw + cw / 2, y: Math.floor(slotOf[i] / cols) * ch + ch / 2 });
  S.smr.gridGeom = { cols, cw, ch, rad, n, order };

  // Adoption and reset-vote edges fade over the next couple of rounds so a
  // paused stepper still shows what the last round's replies caused.
  if (mode === 'rstate' && st.rec) {
    const edges = [...st.rec.adoptions.map(e => [e, '#3dd6c4']),
                   ...st.rec.reset_votes.map(e => [e, '#e0af68'])];
    ctx.lineWidth = 1;
    for (const [[from, to], ecol] of edges) {
      if (from >= n || to >= n) continue;
      const a = at(from), b = at(to);
      ctx.strokeStyle = ecol;
      ctx.globalAlpha = 0.5;
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }

  for (let i = 0; i < n; i++) {
    const { x, y } = at(i);
    ctx.beginPath();
    ctx.arc(x, y, rad, 0, Math.PI * 2);
    ctx.fillStyle = looks[i].color;
    ctx.globalAlpha = looks[i].alpha;
    ctx.fill();
    ctx.globalAlpha = 1;
    if (blocked.has(i)) {
      ctx.beginPath();
      ctx.arc(x, y, rad + 1.5, 0, Math.PI * 2);
      ctx.strokeStyle = '#f7768e';
      ctx.lineWidth = 1;
      ctx.stroke();
    }
    // Dashed ring = blocked by hand, so an operator can tell their own
    // intervention from the schedule's sample.
    if (S.smr.manualBlocks.has(i)) {
      ctx.beginPath();
      ctx.arc(x, y, rad + 3, 0, Math.PI * 2);
      ctx.strokeStyle = '#f7768e'; ctx.lineWidth = 1; ctx.setLineDash([2, 2]);
      ctx.stroke(); ctx.setLineDash([]);
    }
    if (S.smr.pin === i) {
      ctx.beginPath();
      ctx.arc(x, y, rad + 4.5, 0, Math.PI * 2);
      ctx.strokeStyle = '#cfd6df'; ctx.lineWidth = 1.4; ctx.stroke();
    }
  }
  $('smrNetNote').textContent = '';
  $('smrNetLegend').innerHTML = LEGENDS[mode](st) + LEGEND_TAIL;
  renderDrill();
}

/* ---------- node drilldown ---------- */

for (const b of document.querySelectorAll('#smrGridTabs .ct')) {
  b.addEventListener('click', () => {
    S.smr.gridMode = b.dataset.grid;
    if (S.smr.report) renderSmr();
  });
}
$('srvSort').addEventListener('click', () => {
  S.smr.gridSort = !S.smr.gridSort;
  if (S.smr.report) renderSmr();
});
$('srvWatchX').addEventListener('click', () => selectCmd(null));

$('smrNet').addEventListener('click', ev => {
  const geom = S.smr.gridGeom;
  if (!geom || !(S.smr.live || S.smr.handle) || !isRec()) return;
  const r = $('smrNet').getBoundingClientRect();
  const col = Math.floor((ev.clientX - r.left) / geom.cw);
  const row = Math.floor((ev.clientY - r.top) / geom.ch);
  const slot = row * geom.cols + col;
  if (slot < 0 || slot >= geom.n) return;
  const node = geom.order[slot];
  S.smr.pin = S.smr.pin === node ? null : node;
  S.smr.detail = null;
  if (S.smr.pin === null) renderSmr(); else post({ type: 'smr-node-detail', node });
});

function renderDrill() {
  const panel = $('smrDrill');
  const pin = S.smr.pin;
  if (!isRec() || pin === null) { panel.classList.add('hidden'); return; }
  panel.classList.remove('hidden');
  const d = S.smr.detail;
  if (!d) { panel.innerHTML = `<div class="drillHead"><b>node ${pin}</b><span>loading…</span></div>`; return; }
  const R = ['no-reset', 'reset', '⊥'][d.r] || '?';
  const diverged = d.s_hash !== d.checkpoint_s_hash;
  const blocked = S.smr.manualBlocks.has(pin);
  panel.innerHTML =
    `<div class="drillHead"><b>node ${d.node}</b><span>R ${R}</span>` +
    `<span style="flex:1"></span>` +
    `<button class="drillBtn blk${blocked ? ' on' : ''}" id="drillBlock">⌖ ${blocked ? 'release' : 'block'}</button>` +
    `<button class="drillBtn on" id="drillUnpin">unpin</button></div>` +
    row('log |L|', d.log_len === null ? '⊥' : d.log_len) +
    row('executed', d.executed_len) +
    row('checkpoint', `W ${d.checkpoint_window} · |P| ${d.checkpoint_p_len === null ? '⊥' : d.checkpoint_p_len}`) +
    row('S hash', d.s_hash, diverged) +
    row('C.S hash', d.checkpoint_s_hash, diverged);
  panel.querySelector('#drillUnpin').addEventListener('click', () => {
    S.smr.pin = null; S.smr.detail = null; renderSmr();
  });
  panel.querySelector('#drillBlock').addEventListener('click', () => {
    post({ type: 'smr-live-block', node: pin, blocked: !blocked });
  });
}
function row(k, v, diverged = false) {
  return `<div class="drillRow${diverged ? ' diverged' : ''}"><span class="k">${k}</span><span class="v">${v}</span></div>`;
}

function renderMetrics(rep) {
  $('auditCnt').textContent = String(rep.commands.length);
}

function renderAudit(rep) {
  const box = $('smrAudit');
  box.innerHTML = '';
  if (!rep.commands.length) {
    box.innerHTML = '<div class="hint">no commands — add injections or submit one live</div>';
    return;
  }
  const sel = selectedCmd(rep);
  const scope = $('smrAuditScope').value;
  const cap = scope === 'all' ? rep.commands.length : +scope;
  const from = Math.max(0, rep.commands.length - cap);
  if (from > 0) {
    const done = rep.commands.slice(0, from).filter(c => c.status === 'Complete').length;
    const more = document.createElement('div');
    more.className = 'auditIntro';
    more.textContent = `${from} earlier commands (${done} complete) — widen the scope above or see the report export`;
    box.appendChild(more);
  }
  rep.commands.forEach((cmd, i) => {
    if (i < from && i !== sel) return;
    const b = BADGE[cmd.status];
    const card = document.createElement('div');
    card.className = 'auditCard' + (sel === i ? ' sel open' : rep.commands.length <= 3 ? ' open' : '');
    const tm = cmd.prefix_fixed_round !== null && cmd.all_logs_round !== null
      ? cmd.prefix_fixed_round - cmd.all_logs_round : null;
    const lms = [`<span class="lm">injected r${cmd.injection_round}</span>`];
    if (cmd.delivered_round !== null) lms.push(`<span class="lm">delivered r${cmd.delivered_round}</span>`);
    if (cmd.all_logs_round !== null) lms.push(`<span class="lm b">T_B r${cmd.all_logs_round}</span>`);
    if (cmd.prefix_fixed_round !== null)
      lms.push(`<span class="lm e">T_E r${cmd.prefix_fixed_round}${tm !== null ? ' · T_M ' + tm : ''}</span>`);
    if (cmd.committed_ack_round !== null) lms.push(`<span class="lm ack">ack r${cmd.committed_ack_round}</span>`);
    const attRow = a => {
      const oc = OC[a.outcome] || ['oc-ignored', a.outcome];
      return `<div class="att"><span class="r">r${a.round}</span><span class="nd">#${a.target}</span><span class="oc ${oc[0]}">${oc[1]}</span></div>`;
    };
    // runs of ≥3 consecutive ignored resends collapse to one row — the §5
    // resend-until-ack client would flood the trail otherwise
    const parts = [];
    for (let j = 0; j < cmd.attempts.length;) {
      let k = j;
      while (k < cmd.attempts.length && cmd.attempts[k].outcome === 'Ignored') k++;
      if (k - j >= 3) {
        const [cls, label] = OC.Ignored;
        parts.push(`<div class="att attRun"><span class="r">×${k - j}</span><span class="nd">r${cmd.attempts[j].round}–r${cmd.attempts[k - 1].round}</span><span class="oc ${cls}">${label}</span></div>`);
        j = k;
      } else {
        parts.push(attRow(cmd.attempts[j]));
        j++;
      }
    }
    const atts = parts.join('');
    card.innerHTML =
      `<div class="auditHead"><span class="sw" style="background:${cmdColor(i)}"></span>` +
      `<span class="auditLabel mono">${cmdLabel(cmd)}</span>` +
      `<span class="badge ${b[0]}">${b[1]}</span><span class="chev">▸</span></div>` +
      `<div class="auditBody">` +
      `<div class="lmRow">${lms.join('')}</div>` +
      `<div class="ampRow">amp-receivers: <b>${cmd.amp_receivers.length || '—'}</b></div>` +
      `<div class="attList"><div class="attCap">delivery attempts</div>${atts}</div>` +
      `</div>`;
    card.querySelector('.auditHead').addEventListener('click', () => {
      selectCmd(S.smr.sel === i && card.classList.contains('open') ? null : i);
    });
    box.appendChild(card);
  });
}

// one starter row so the form is runnable as-is
injRow(2);
syncStrategy();
window.addEventListener('resize', () => { if (S.smr.report && S.view === 'smr') renderSmr(); });
export { renderSmr };
