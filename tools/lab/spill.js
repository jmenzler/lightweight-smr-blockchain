// Canonical-spill viewer: a LEAN recovery run's committed history, which no
// server in that run still holds.
//
// The reader is Rust. This file does file I/O and rendering and nothing else —
// it never parses the spill format, never hashes an entry, and never builds a
// forest. `SpillView` in sim-wasm is the one implementation of all three, the
// same code the native observer's diagnosis path runs.
//
// A spill is boundary-granular by construction: it records what each server
// had committed at each T-window boundary, not what happened between them. The
// panels that need sub-boundary state are therefore not "not implemented" —
// they are unreconstructable, and each one says so rather than rendering blank.
import { S, $ } from './state.js';
import { post } from './engine.js';
import { onChainSnapshot, resetCertsDrawer } from './smr-certs.js';
import { setView } from './render.js';
import { openForest, closeDrawer, renderDrawer } from './smr-drawer.js';

S.spill = null;

// Every SMR panel a spill cannot feed: the panel is taken down and its reason
// written here. Leaving them up would render four empty boxes, and an empty
// chart invites the reader to take absence for a measurement — which is the
// one thing a viewer over a run that deliberately forgot must never do.
const ABSENT = [
  ['smrViewport', 'Per-round timeline', 'a spill records T-window boundaries, not the rounds between them'],
  ['smrNetBox', 'Servers — log lengths, ⊥ and R states', 'per-round server state lives in the metrics CSV, not the spill'],
  ['smrViewport', 'Spread and position curves', 'lean runs release a command’s curve once it settles — nothing retains them'],
  ['smrViewport', 'Prefix agreement', 'it is a per-round ratio, and a spill holds no per-round state'],
  ['smrViewport', 'Blocked-server lane', 'who was blocked in which round is schedule state, not committed history'],
];

/// Route a dropped or loaded file: a spill is JSONL, so a whole-file
/// `JSON.parse` throws. Sniff the first line instead, exactly as the spec/trace
/// discrimination in form.js does.
export function looksLikeSpill(text) {
  const first = text.slice(0, text.indexOf('\n') === -1 ? text.length : text.indexOf('\n'));
  return first.includes('"kind":"header"') || first.includes('"kind": "header"');
}

export function loadSpill(text) {
  post({ type: 'spill-load', text });
  $('smrStatus').textContent = 'reading spill…';
}

export function onSpillReady(m) {
  const out = JSON.parse(m.json);
  if (out.error) {
    S.spill = null;
    $('spillPanel').classList.add('hidden');
    $('smrStatus').textContent = 'spill: ' + out.error;
    return;
  }
  S.spill = { ...out, b: out.boundaries.length - 1, node: 0 };
  resetCertsDrawer();
  S.smr.chain = null;
  setView('smr');
  $('smrRun').classList.remove('hidden');
  $('spillPanel').classList.remove('hidden');
  $('smrCtl').classList.add('hidden');
  hideUnfeedable();
  buildNodeOptions(out.n);
  $('spillScrub').max = String(Math.max(0, out.boundaries.length - 1));
  $('spillScrub').value = String(S.spill.b);
  renderSpill();
  openForest('canonical spill');
  requestChain();
}

export function onSpillChain(m) {
  const chain = JSON.parse(m.json);
  if (chain.error) {
    $('spillChainNote').textContent = 'chain unavailable: ' + chain.error;
    return;
  }
  $('spillChainNote').textContent = '';
  // A full snapshot with no merge cascade: the cascade is a live-session
  // affordance, and animating one because a scrubber moved would claim a
  // boundary just happened when the viewer simply jumped to it.
  S.smr.chain = { m: chain.m, peaks: chain.peaks, merges: [], entries: chain.entries };
  onChainSnapshot(S.smr.chain);
  renderDrawer();
}

function buildNodeOptions(n) {
  const sel = $('spillNode');
  sel.innerHTML = '';
  for (let i = 0; i < n; i++) {
    const o = document.createElement('option');
    o.value = String(i);
    o.textContent = 'server ' + i;
    sel.appendChild(o);
  }
  sel.value = '0';
}

function requestChain() {
  if (!S.spill) return;
  post({ type: 'spill-chain', boundary: S.spill.b, node: S.spill.node });
}

function renderSpill() {
  const sp = S.spill;
  if (!sp) return;
  const b = sp.boundaries[sp.b];

  const trust = $('spillTrust');
  if (!sp.verified) {
    trust.className = 'badge badge-dead';
    trust.textContent = 'UNVERIFIED — ' + (sp.verify_error || 'digest mismatch');
  } else if (sp.diverged) {
    trust.className = 'badge badge-dead';
    trust.textContent = 'diverged — the run stopped being one committed order; records end here';
  } else if (!sp.complete) {
    trust.className = 'badge badge-run';
    trust.textContent = 'incomplete — no trailer, so the run was killed or aborted mid-flight';
  } else {
    trust.className = 'badge badge-ok';
    trust.textContent = 'verified against the run’s own digest chain';
  }

  $('spillProv').textContent =
    `n ${sp.n} · seed ${sp.seed} · T ${sp.t_window} · ${sp.proto} · commit ${String(sp.commit).slice(0, 8)}${sp.dirty ? ' (dirty)' : ''}`;

  if (!b) {
    $('spillWhere').textContent = 'no boundary records — the run never reached one';
    $('spillLens').textContent = '';
    return;
  }
  $('spillWhere').textContent =
    `window ${b.w} · round ${b.round} · committed ${b.committed} · release line ${b.release} · digest ${b.digest}`;

  const len = b.lens[sp.node];
  const off = b.offsets[sp.node];
  $('spillLens').textContent =
    `server ${sp.node}: executed ${len}, forgetting to ${off} at this boundary` +
    ` · population ${Math.min(...b.lens)}–${Math.max(...b.lens)}`;

  const list = $('spillAbsent');
  list.innerHTML = '';
  for (const [id, what, why] of ABSENT) {
    const li = document.createElement('li');
    li.innerHTML = `<b>${what}</b> — ${why}`;
    list.appendChild(li);
  }
}

// Taken down while a spill is open, restored to exactly the visibility they
// had when it closes — several of them start hidden anyway, so "unhide all"
// would leave the live session showing panels it had not opened.
let restore = null;
function hideUnfeedable() {
  // The run form goes with them: opening a spill is entering a session, and a
  // form offering to start a new one on top of it is just noise.
  const ids = [...new Set(ABSENT.map(([id]) => id)), 'smrFormCard'];
  restore = ids.map(id => [id, $(id).classList.contains('hidden')]);
  for (const id of ids) $(id).classList.add('hidden');
}
function showUnfeedable() {
  if (!restore) return;
  for (const [id, wasHidden] of restore) $(id).classList.toggle('hidden', wasHidden);
  restore = null;
}

function goToBoundary(b) {
  if (!S.spill) return;
  S.spill.b = Math.max(0, Math.min(b, S.spill.boundaries.length - 1));
  $('spillScrub').value = String(S.spill.b);
  renderSpill();
  requestChain();
}
$('spillScrub').addEventListener('input', e => goToBoundary(+e.target.value));
$('spillPrevBtn').addEventListener('click', () => { if (S.spill) goToBoundary(S.spill.b - 1); });
$('spillNextBtn').addEventListener('click', () => { if (S.spill) goToBoundary(S.spill.b + 1); });
$('spillNode').addEventListener('change', e => {
  if (!S.spill) return;
  S.spill.node = Math.max(0, +e.target.value | 0);
  renderSpill();
  requestChain();
});
$('spillCloseBtn').addEventListener('click', () => {
  S.spill = null;
  S.smr.chain = null;
  $('spillPanel').classList.add('hidden');
  showUnfeedable();
  resetCertsDrawer();
  closeDrawer();
  if (!S.smr.ctx) $('smrRun').classList.add('hidden');
  else $('smrCtl').classList.remove('hidden');
});

// Deep link: `?spill=<path served beside the lab>` opens one straight away.
// A spill arrives as a path under .runs/spills, not as a file sitting in a
// picker, so this is the ingest a researcher actually has. Same-origin only —
// the lab fetches nothing it was not served alongside.
const wanted = new URLSearchParams(location.search).get('spill');
if (wanted) {
  const url = new URL(wanted, location.href);
  if (url.origin !== location.origin) {
    $('smrStatus').textContent = 'spill link refused: not served beside the lab';
  } else {
    fetch(url)
      .then(r => (r.ok ? r.text() : Promise.reject(new Error('HTTP ' + r.status))))
      .then(loadSpill)
      .catch(e => { $('smrStatus').textContent = 'spill fetch failed: ' + e; });
  }
}
