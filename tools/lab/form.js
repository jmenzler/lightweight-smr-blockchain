// run form: spec building, presets, scenario import/export
import { S, $, clamp01 } from './state.js';
import { looksLikeSpill, loadSpill } from './spill.js';
import { PMF } from './pmf-editor.js';
import { programBeta } from './smr-derive.js';
import { openTrace, runBatchSpec } from './live.js';

/* ---------- run form ---------- */

const VALUE_COUNT_CAP = { random: 4294967296, even: 200000 };
function syncFormRows() {
  const init = $('runInit').value;
  $('rowSplitFrac').classList.toggle('hidden', init !== 'split');
  $('rowRandK').classList.toggle('hidden', !(init === 'random' || init === 'even'));
  $('rowPmf').classList.toggle('hidden', init !== 'weighted');
  if (VALUE_COUNT_CAP[init]) {
    $('runRandK').max = VALUE_COUNT_CAP[init];
    $('runRandK').title = 'up to ' + VALUE_COUNT_CAP[init].toLocaleString('en-US') + ' values for this init';
  }
  if (init === 'weighted') PMF.draw();
}
$('runInit').addEventListener('change', syncFormRows);
$('runRandK').addEventListener('input', () => {
  const cap = VALUE_COUNT_CAP[$('runInit').value];
  if (cap && +$('runRandK').value > cap) $('runRandK').value = cap;
});

function baseInitSpec(kind) {
  if (kind === 'split') return { kind: 'split', fraction: clamp01(+$('runSplitFrac').value || 0) };
  if (kind === 'random') return { kind: 'uniform_random', k: Math.min(Math.max(1, Math.round(+$('runRandK').value || 4)), VALUE_COUNT_CAP.random) };
  if (kind === 'even') return { kind: 'even_split', values: Math.min(Math.max(1, Math.round(+$('runRandK').value || 4)), VALUE_COUNT_CAP.even) };
  if (kind === 'weighted') {
    const weights = PMF.weights();
    const spec = { kind: 'weighted', weights };
    const range = PMF.range();
    if (range > weights.length) spec.range = range;
    return spec;
  }
  return { kind: 'distinct' };
}
function buildInitSpec() {
  const base = baseInitSpec($('runInit').value);
  const useful = clamp01(+$('runUseful').value);
  if (useful < 1) {
    return { kind: 'with_undecided', useful_fraction: useful, inner: base };
  }
  return base;
}
/* ---------- blocking strategy ---------- */

// fresh, sticky and a scheduled program run in a live session: the live path
// draws exactly the stream the batch engine draws for them. Permanent and the
// adaptive strategies need draws or node state the live path does not have, so
// they run through the batch engine and open as a finished run.
export const LIVE_STRATEGIES = ['fresh', 'sticky', 'program'];
const STRATEGY_HINT = {
  fresh: 'A new random set of nodes is blocked every round (≈ Lemma 2.2’s adversary); the live slider changes the fraction.',
  sticky: 'A persistent targeted set follows the live slider; the percentage here is never-stopping background noise under it.',
  permanent: 'One fixed set for the whole run (≈ Lemma 2.3’s adversary). The engine draws it before round 1, so this runs as a finished batch run.',
  program: 'Blocked fraction per round segment; the live slider takes over from the round you grab it.',
  adaptive0: 'Blocks the value holders, reading the state one round later than Definition 1.1 allows: a stress adversary outside the paper’s model. Runs as a finished batch run.',
  adaptiveAlpha: 'The Definition 1.1 α-late adversary; α = 1 is the paper’s 1-late adversary. Runs as a finished batch run.',
};
export const strategy = () => $('svStrategy').value;

export function syncStrategy() {
  const st = strategy();
  $('svRowBeta').classList.toggle('hidden', st === 'program');
  $('svRowAlpha').classList.toggle('hidden', st !== 'adaptiveAlpha');
  $('svRowProgram').classList.toggle('hidden', st !== 'program');
  $('svBetaLabel').textContent = st === 'sticky' ? 'background blocked % of n'
    : LIVE_STRATEGIES.includes(st) ? 'blocked % of n (the live slider starts here)' : 'blocked % of n';
  $('svStrategyHint').textContent = STRATEGY_HINT[st] ?? '';
}
$('svStrategy').addEventListener('change', syncStrategy);

function progRow(from = 10, to = 60, pct = 30) {
  const row = document.createElement('div');
  row.className = 'progRow';
  row.innerHTML = `
    <input class="fInput progFrom" type="number" min="1" value="${from}" />
    <input class="fInput progTo" type="number" min="2" value="${to}" />
    <input class="fInput progPct" type="number" min="0" max="60" value="${pct}" />
    <button class="injDel" title="remove">✕</button>`;
  row.querySelector('.injDel').addEventListener('click', () => row.remove());
  $('svProgRows').appendChild(row);
}
$('svProgAdd').addEventListener('click', () => progRow());
progRow();
syncStrategy();

export function readProgram() {
  return [...document.querySelectorAll('#svProgRows .progRow')].map(row => ({
    from: Math.max(1, Math.round(+row.querySelector('.progFrom').value || 1)),
    to: Math.round(+row.querySelector('.progTo').value || 1),
    beta: Math.min(Math.max(+row.querySelector('.progPct').value || 0, 0), 60),
  })).filter(seg => seg.to > seg.from);
}

function scheduleSpec(st, beta, maxRounds) {
  const fraction = beta / 100;
  if (st === 'permanent') return { kind: 'permanent', fraction };
  if (st === 'adaptive0') return { kind: 'adaptive_0late', fraction, policy: 'block_holders' };
  if (st === 'adaptiveAlpha') return { kind: 'adaptive_alpha_late', fraction, alpha: Math.max(1, Math.round(+$('runAlpha').value || 1)), policy: 'block_holders' };
  if (st === 'program') {
    const program = readProgram();
    return { kind: 'per_round_fractions', fractions: Array.from({ length: maxRounds }, (_, i) => (programBeta(program, i + 1) ?? 0) / 100) };
  }
  return { kind: 'fresh_per_round', fraction };
}

export function buildSpec() {
  const n = Math.min(Math.max(+$('runN').value || 1000, 10), 200000);
  const beta = Math.min(Math.max(+$('runBeta').value || 0, 0), 60);
  const maxRounds = Math.min(Math.max(+$('runMax').value || 1000, 1), 5000);
  return {
    n,
    seed: +$('runSeed').value || 0,
    k: Math.round(+$('runK').value || 6),
    ell: Math.round(+$('runEll').value || 3),
    init: buildInitSpec(),
    max_rounds: maxRounds,
    schedule: scheduleSpec(strategy(), beta, maxRounds),
  };
}
export function validateSpec(spec) {
  if (!(spec.k > 1 && spec.ell > 1 && spec.k >= spec.ell && spec.ell % 2 === 1)) {
    return 'config needs k, ℓ > 1, k ≥ ℓ, ℓ odd';
  }
  return null;
}
/* ---------- E2 presets (prereg 2026-07-21-e2-three-regime-bracket) ---------- */
function applyPreset(p) {
  $('runInit').value = p.init;
  $('runUseful').value = p.useful !== undefined ? p.useful : 1;
  $('runSplitFrac').value = 0.5;
  $('svStrategy').value = p.strategy;
  syncStrategy();
  $('runBeta').value = p.beta;
  $('runMax').value = p.maxRounds;
  syncFormRows();
  $('runStatus').textContent = p.label + ' preset loaded — press Start';
}
$('presetA').addEventListener('click', () => applyPreset({
  label: 'A collapse', init: 'split', useful: 0.30,
  strategy: 'fresh', beta: 0, maxRounds: 200,
}));
$('presetB').addEventListener('click', () => applyPreset({
  label: 'B converge-hold', init: 'split', useful: 5 / 9,
  strategy: 'fresh', beta: 10, maxRounds: 500,
}));
$('presetC').addEventListener('click', () => applyPreset({
  label: 'C death spiral', init: 'split',
  strategy: 'permanent', beta: 30, maxRounds: 500,
}));

/* ---------- scenario import / export ---------- */
export function downloadJson(name, text) {
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  a.download = name;
  a.click();
  URL.revokeObjectURL(a.href);
}
$('exportSpecBtn').addEventListener('click', () => {
  downloadJson('scenario.json', JSON.stringify(buildSpec(), null, 2));
});

/* Fill the form when the spec is expressible; false = run it directly. */
function fillFormFromSpec(spec) {
  const sched = spec.schedule || { kind: 'fresh_per_round', fraction: 0 };
  const FORM_KINDS = { fresh_per_round: 'fresh', permanent: 'permanent', adaptive_0late: 'adaptive0', adaptive_alpha_late: 'adaptiveAlpha' };
  if (!(sched.kind in FORM_KINDS)) return false;
  const setBase = (init, prefix) => {
    if (init.kind === 'split') { $(prefix).value = 'split'; $('runSplitFrac').value = init.fraction; return true; }
    if (init.kind === 'uniform_random') { $(prefix).value = 'random'; $('runRandK').value = init.k; return true; }
    if (init.kind === 'distinct') { $(prefix).value = 'distinct'; return true; }
    if (init.kind === 'even_split') { $(prefix).value = 'even'; $('runRandK').value = init.values; return true; }
    if (init.kind === 'weighted') {
      $(prefix).value = 'weighted';
      $('pmfRange').value = init.range || init.weights.length;
      PMF.imported = init.weights;
      return true;
    }
    return false;
  };
  if (spec.init.kind === 'with_undecided') {
    $('runUseful').value = spec.init.useful_fraction;
    if (!setBase(spec.init.inner, 'runInit')) return false;
  } else {
    $('runUseful').value = 1;
    if (!setBase(spec.init, 'runInit')) return false;
  }
  $('runN').value = spec.n;
  $('runSeed').value = spec.seed;
  $('runK').value = spec.k;
  $('runEll').value = spec.ell;
  $('runMax').value = spec.max_rounds;
  $('svStrategy').value = FORM_KINDS[sched.kind];
  if (sched.alpha) $('runAlpha').value = sched.alpha;
  syncStrategy();
  $('runBeta').value = Math.round((sched.fraction || 0) * 100);
  syncFormRows();
  return true;
}
// A single-value scenario fills the form when the form can express it,
// otherwise it runs directly.
export function importSingleSpec(parsed) {
  if (fillFormFromSpec(parsed)) {
    $('runStatus').textContent = 'scenario loaded — press Start';
  } else {
    runBatchSpec(parsed);
  }
}
$('importSpecBtn').addEventListener('click', () => {
  const inp = document.createElement('input');
  inp.type = 'file';
  inp.accept = '.json,.jsonl,application/json';
  inp.onchange = () => {
    const f = inp.files[0];
    if (!f) return;
    f.text().then(t => {
      // JSONL: a whole-file JSON.parse would throw, so sniff the first line
      // first, exactly as the trace-versus-spec discrimination below does.
      if (looksLikeSpill(t)) { loadSpill(t); return; }
      const parsed = JSON.parse(t);
      if (Array.isArray(parsed.rounds)) { openTrace(parsed); return; }
      importSingleSpec(parsed);
    }).catch(e => { $('runStatus').textContent = 'import failed: ' + e; });
  };
  inp.click();
});
