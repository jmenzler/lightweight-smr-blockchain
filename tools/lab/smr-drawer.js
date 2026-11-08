// Drawers under the timeline, at most one open: the block drawer (one chain
// block: its entries, the proof path of one entry through the Merkle forest,
// the whole forest, certificates) and the spread drawer (how fast commands
// reach every server, and where a command sits in the logs). Everything shown
// comes from the report or from the engine's forest feed; the forest feed
// carries peaks and roots only, so the proof path draws structure and never a
// hash value it was not given.
import { S, $ } from './state.js';
import { forestView, certsLoading, entryLabel, tallyFor, cmdIndexFor } from './smr-certs.js';
import { chainBlocks, spreadStats, lifelines, shownMargin, committedOrder, committedEntries, displacements, withinShare, fifoCheck, spreadCurve, entryMark, slowAbove, listWindow, syncedEntry, pickedEntry, cmdName as nameOf, isPool } from './smr-derive.js';
import { histSvg, dispColor, signedInt } from './smr-hist.js';
import { useViewportHandlers, viewRange } from './smr-viewport.js';
import { escapeHtml } from './tooltip.js';
import { post } from './engine.js';

const MAX_CHIPS = 400;
const WHOLE_TREE_LEAVES = 32;
// beyond this an SVG of every node stops being readable or cheap to draw
const FULL_TREE_MAX_LEAVES = 4096;
// engine-recomputed node hashes per tree, keyed by start:height; a perfect subtree never changes
const trees = new Map();
const treeKey = peak => `${peak.start}:${peak.height}`;
const TEAL = '#3dd6c4', AMBER = '#e0af68';
const d = { open: null, block: null, view: 'this', tab: 'dist', entry: null };

useViewportHandlers({
  openBlock: block => { if (block === null) closeDrawer(); else openBlock(block); },
  openSpread: () => openSpread(),
  selectCmd: i => selectCmd(i),
});

// smr.js asks the engine for the forest feed when the block drawer opens: steps skip it while closed.
let onBlockDrawerOpen = () => {};
export function useBlockDrawerOpen(fn) { onBlockDrawerOpen = fn; }
export const blockDrawerOpen = () => d.open === 'block';

// smr.js owns command selection, so a pick here re-renders the audit trail too, paused or not.
let selectCmd = i => { S.smr.sel = i; render(); };
export function useSelectCmd(fn) { selectCmd = fn; }

function openBlock(block) {
  const wasOpen = blockDrawerOpen();
  d.open = 'block';
  d.block = block;
  d.entry = null;
  if (d.view === 'forest' || !wasOpen) d.view = 'this';
  if (!wasOpen) onBlockDrawerOpen();
  render();
}
function openSpread() {
  if (d.open === 'spread') { closeDrawer(); return; }
  d.open = 'spread';
  d.tab = 'dist';
  S.smr.blockSel = null;
  render();
}
// The spill viewer has a forest but no report: show it without a block.
export function openForest(title) {
  d.open = 'block';
  d.block = null;
  d.view = 'forest';
  d.title = title;
  render();
}
// Closing the block drawer deselects the block and any command picked in it.
export function closeDrawer() {
  const wasBlock = d.open === 'block';
  d.open = null;
  S.smr.blockSel = null;
  if (wasBlock && S.smr.sel !== null) selectCmd(null);
  else render();
}
export function resetDrawer() {
  d.open = null; d.block = null; d.entry = null; d.title = null;
  trees.clear();
  $('smrDrawer').classList.add('hidden');
}
$('drawerClose').addEventListener('click', closeDrawer);
export const drawerIsOpen = () => d.open !== null;
window.addEventListener('keydown', e => { if (e.key === 'Escape' && d.open) closeDrawer(); });
$('drMeasure').addEventListener('change', render);
$('bdProofViz').addEventListener('click', e => {
  const btn = e.target.closest('[data-tree]');
  if (!btn) return;
  const [start, height] = btn.dataset.tree.split(':').map(Number);
  trees.set(btn.dataset.tree, { loading: true });
  post({ type: 'smr-merkle-tree', start, height });
  render();
});
export function onMerkleTree(m) {
  const out = JSON.parse(m.json);
  trees.set(`${m.start}:${m.height}`, out.error ? { error: out.error } : { levels: out.levels });
  render();
}

export function renderDrawer() { render(); }

function setTabs(tabs, current, onPick) {
  const box = $('drawerTabs');
  box.innerHTML = tabs.map(([key, label]) => `<button class="ct${key === current ? ' on' : ''}" data-k="${key}">${label}</button>`).join('');
  for (const b of box.querySelectorAll('.ct')) b.addEventListener('click', () => onPick(b.dataset.k));
}

function render() {
  const rep = S.smr.report, c = S.smr.ctx;
  const forestOnly = d.open === 'block' && d.block === null;
  $('smrDrawer').classList.toggle('hidden', !d.open || (!forestOnly && (!rep || !c)));
  if (!d.open || (!forestOnly && (!rep || !c))) return;
  if (forestOnly) { renderForestOnly(); return; }
  $('blockDrawer').classList.toggle('hidden', d.open !== 'block');
  $('spreadDrawer').classList.toggle('hidden', d.open !== 'spread');
  if (d.open === 'block') renderBlock(rep, c); else renderSpread(rep, c);
}

/* ---------- block drawer ---------- */

function renderForestOnly() {
  $('blockDrawer').classList.remove('hidden');
  $('spreadDrawer').classList.add('hidden');
  $('drawerKind').textContent = 'Committed sequence';
  $('drawerTitle').textContent = d.title || '';
  $('drawerTabs').innerHTML = '';
  $('drawerWarn').classList.add('hidden');
  $('bdThis').classList.add('hidden');
  $('bdForest').classList.remove('hidden');
  $('bdCerts').classList.add('hidden');
  $('bdNoForest').classList.toggle('hidden', !!forestView());
  $('bdNoForest').textContent = 'loading the committed sequence…';
}

function renderBlock(rep, c) {
  const block = chainBlocks(rep, c.proto, c.T).find(b => b.m === d.block.m);
  if (!block) { closeDrawer(); return; }
  const fv = forestView();
  $('drawerKind').textContent = 'Block';
  $('drawerTitle').textContent = `${block.label} · ${block.size} entr${block.size === 1 ? 'y' : 'ies'}` +
    (block.size ? ` · #${block.from + 1}–#${block.from + block.size}` : '') +
    (fv ? ` · forest over m = ${fv.m}` : '');
  const tabs = [['this', 'Entries'], ['forest', 'Proof']];
  if (c.proto === 'recovery' || c.proto === 'compact') tabs.splice(1, 0, ['life', 'Lifelines'], ['order', 'Order']);
  if (!tabs.some(([k]) => k === d.view)) d.view = 'this';
  setTabs(tabs, d.view, k => { d.view = k; render(); });
  $('bdThis').classList.toggle('hidden', d.view !== 'this');
  $('bdLife').classList.toggle('hidden', d.view !== 'life');
  $('bdOrder').classList.toggle('hidden', d.view !== 'order');
  $('bdForest').classList.toggle('hidden', d.view !== 'forest');
  const noForest = !fv;
  $('bdNoForest').classList.toggle('hidden', !noForest);
  $('bdNoForest').textContent = certsLoading()
    ? 'loading the committed sequence…'
    : 'No committed-sequence feed for this run: a spec (batch) run records the report only. Open a live session to see the forest and its certificates.';
  renderWarn(c, fv);
  if (d.view === 'life' || d.view === 'order') $('bdCerts').classList.add('hidden');
  placeCerts(d.view === 'this');
  if (d.view === 'this') { renderChips(rep, c, block, fv); renderProof(rep, c, fv, block); }
  if (d.view === 'life') renderLifelines(rep, c, block, fv);
  if (d.view === 'order') renderOrder(rep, c, block, fv);
}

// Certificates exist for compact, and for recovery only with the thesis's own
// resend-until-acked construction; the paper proves Theorem 6 for compact.
function renderWarn(c, fv) {
  const warn = $('drawerWarn');
  const note = $('bdCertNote');
  if (c.proto === 'recovery' && !c.certsRec) {
    warn.classList.remove('hidden');
    warn.dataset.tip = 'Certificates are off for this recovery run: turn certificates on in the setup form to verify commands. The proof path below shows structure only.';
    note.textContent = '';
    $('bdCerts').classList.add('hidden');
    return;
  }
  warn.classList.add('hidden');
  $('bdCerts').classList.toggle('hidden', !fv);
  note.textContent = c.proto === 'recovery' ? 'certificates over recovery — thesis construction, not the paper’s proven guarantee (Theorem 6 covers compact only)' : '';
}

// The certificates section sits under the proof in the Entries tab and under the forest otherwise.
const certsHome = { parent: null, next: null };
function placeCerts(inEntries) {
  const el = $('bdCerts');
  if (!certsHome.parent) { certsHome.parent = el.parentNode; certsHome.next = el.nextSibling; }
  if (inEntries) $('bdCertsSlot').appendChild(el);
  else if (el.parentNode !== certsHome.parent) certsHome.parent.insertBefore(el, certsHome.next);
}

/* entries: one row per committed entry, built only near the viewport */

const ROW = 20;
const list = { block: null, rows: null, bound: false, shown: null };

function renderChips(rep, c, block, fv) {
  const box = $('bdChips');
  if (!block.size) {
    box.innerHTML = '<div class="bdDim" style="padding:8px">nothing committed in this block' +
      (block.state === 'rollback' ? ': the boundary rolled servers back instead' : '') + '</div>';
    list.rows = null;
    return;
  }
  if (!fv || !fv.entries) {
    // no forest feed: only the report's client commands are known, not positions
    box.innerHTML = '<div class="bdDim" style="padding:8px">entry positions need the live forest feed; client commands in this block: ' +
      (block.cmds.length ? block.cmds.map(i => escapeHtml(cmdName(rep.commands[i]))).join(', ') : 'none') + '</div>';
    list.rows = null;
    return;
  }
  const last = Math.min(block.from + block.size, fv.entries.length);
  const limit = slowAbove(rep, c.proto);
  const rows = [];
  for (let p = block.from + 1; p <= last; p++) {
    const ent = fv.entries[p - 1];
    const i = ent.kind === 'cmd' ? cmdIndexFor(ent) : -1;
    const cert = i >= 0 && fv.mode !== 'none' ? tallyFor(ent.client, ent.sn) : null;
    rows.push({ pos: p, ent, cmd: i, cert, mark: entryMark(ent, i >= 0 ? rep.commands[i] : null, c.proto, limit, cert), label: entryLabel(ent) });
  }
  const changedBlock = list.block !== block.m;
  list.block = block.m;
  list.rows = rows;
  d.entry = syncedEntry(rows, S.smr.sel, d.entry);
  if (changedBlock) box.scrollTop = 0;
  if (!list.bound) {
    list.bound = true;
    box.addEventListener('scroll', () => paintRows());
    box.addEventListener('click', e => {
      const row = e.target.closest('.bdRow');
      if (row) pickEntry(+row.dataset.pos, true);
    });
    box.addEventListener('keydown', e => {
      if (!list.rows || (e.key !== 'ArrowDown' && e.key !== 'ArrowUp')) return;
      e.preventDefault();
      const k = list.rows.findIndex(x => x.pos === d.entry);
      const next = k < 0 ? 0 : Math.max(0, Math.min(list.rows.length - 1, k + (e.key === 'ArrowDown' ? 1 : -1)));
      pickEntry(list.rows[next].pos, false);
    });
  }
  if (d.entry !== list.shown) {
    list.shown = d.entry;
    keepInView();
  }
  paintRows();
}

// A click toggles, a key moves; a command entry selects the command everywhere, a no-op or ⊥ entry
// clears that selection.
function pickEntry(pos, toggle) {
  const next = pickedEntry(list.rows, S.smr.sel, d.entry, pos, toggle);
  d.entry = next.entry;
  if (next.select !== undefined) selectCmd(next.select); else render();
}

function keepInView() {
  const box = $('bdChips');
  const k = list.rows.findIndex(x => x.pos === d.entry);
  if (k < 0) return;
  const top = k * ROW, h = box.clientHeight || 460;
  if (top < box.scrollTop) box.scrollTop = top;
  else if (top + ROW > box.scrollTop + h) box.scrollTop = top + ROW - h;
}

const MARK_TIP = { mine: 'a command you injected', slow: 'committed later than the run’s p90', nocert: 'verified: no certificate constructible', plain: '', filler: '' };
function paintRows() {
  const box = $('bdChips');
  const rows = list.rows;
  if (!rows) return;
  const [lo, hi] = listWindow(box.scrollTop, box.clientHeight || 460, ROW, rows.length, 10);
  let html = `<div style="height:${rows.length * ROW}px"></div>`;
  for (let k = lo; k < hi; k++) {
    const r = rows[k];
    const cls = ['bdRow', r.mark, r.pos === d.entry ? 'sel' : ''].filter(Boolean).join(' ');
    const unverified = r.cert && !r.cert.covered ? ` · not covered by the verify @r${r.cert.round}` : '';
    const tip = `${r.label.title} — position ${r.pos}` + (MARK_TIP[r.mark] ? ` · ${MARK_TIP[r.mark]}` : '') + unverified;
    html += `<div class="${cls}" data-pos="${r.pos}" style="top:${k * ROW}px" data-tip="${escapeHtml(tip)}"><span class="pos">#${r.pos}</span><i></i><span>${escapeHtml(r.label.text)}</span></div>`;
  }
  box.innerHTML = html;
}

/* proof path: leaf → peak root, the sibling interval at every level */

function proofFor(pos, peaks) {
  const peak = peaks.find(pk => pos > pk.start && pos <= pk.start + 2 ** pk.height);
  if (!peak) return null;
  const levels = [];
  let lo = pos, hi = pos;
  for (let lvl = 0; lvl < peak.height; lvl++) {
    const span = 2 ** (lvl + 1);
    const rel = Math.floor((lo - peak.start - 1) / span);
    const pLo = peak.start + rel * span + 1, pHi = peak.start + (rel + 1) * span;
    const mid = pLo + span / 2 - 1;
    const sibling = lo <= mid ? [mid + 1, pHi] : [pLo, mid];
    levels.push({ path: [lo, hi], sibling, siblingOnRight: lo <= mid });
    lo = pLo; hi = pHi;
  }
  return { peak, levels };
}
const range = ([a, b]) => (a === b ? `#${a}` : `#${a}–#${b}`);
const count = ([a, b]) => `${b - a + 1} entr${b === a ? 'y' : 'ies'}`;

function renderProof(rep, c, fv, block) {
  const viz = $('bdProofViz'), steps = $('bdSteps');
  if (!fv || d.entry === null) {
    // nothing picked: the forest's trees, those covering this block marked
    const lo = block.from + 1, hi = block.from + block.size;
    const sky = fv && fv.peaks ? `<div class="bdSky">${fv.peaks.map(pk => {
      const covers = pk.start + 1 <= hi && pk.start + 2 ** pk.height >= lo;
      return `<span class="bdSkyBar${covers ? ' on' : ''}" style="width:${Math.max(8, Math.min(120, 2 ** pk.height * 2))}px" data-tip="tree of ${2 ** pk.height} leaves, positions ${range([pk.start + 1, pk.start + 2 ** pk.height])}">${pk.height}</span>`;
    }).join('')}</div>` : '';
    viz.innerHTML = sky + '<div class="bdDim">' + (fv && fv.entries ? 'pick an entry on the left to see its path through the Merkle forest' : '') + '</div>';
    steps.innerHTML = '';
    return;
  }
  const proof = proofFor(d.entry, fv.peaks);
  if (!proof) { viz.innerHTML = '<div class="bdDim">this entry is not covered by the forest yet</div>'; steps.innerHTML = ''; return; }
  const ent = fv.entries ? fv.entries[d.entry - 1] : null;
  viz.innerHTML = proofSvg(d.entry, proof, fv.peaks);
  if (trees.get(treeKey(proof.peak))?.levels) {
    const leaf = viz.querySelector(`circle[data-tip^="${range([d.entry, d.entry])} "]`);
    if (leaf) viz.scrollLeft = Math.max(0, +leaf.getAttribute('cx') - viz.clientWidth / 2);
  }
  const root = String(proof.peak.root_hex).slice(0, 8);
  let html = `<li>hash the entry at position ${d.entry}${ent ? ` (${escapeHtml(entryLabel(ent).title)})` : ''}</li>`;
  proof.levels.forEach(l => {
    html += `<li>combine with the sibling on the ${l.siblingOnRight ? 'right' : 'left'} covering ${range(l.sibling)} (${count(l.sibling)})</li>`;
  });
  html += proof.peak.height === 0
    ? `<li>this entry is its own tree: its leaf is the root <span class="mono">${root}</span></li>`
    : `<li>the result is this tree's root <span class="mono">${root}</span> (root as reported by the engine)</li>`;
  const tree = trees.get(treeKey(proof.peak));
  if (tree?.levels) html = hashSteps(d.entry, proof, tree.levels, ent);
  else html += '<li class="bdDim">intermediate hashes are recomputed by the engine on request: render full tree</li>';
  if (ent && ent.kind === 'cmd' && fv.mode !== 'none') {
    const t = tallyFor(ent.client, ent.sn);
    html += `<li>certificate: ${t === null ? 'press verify everywhere to check it against every server'
      : !t.covered ? `not covered by the verify @r${t.round} (committed later, or not a tracked client)`
        : t.tally ? `accepted by ${t.tally.accepted} of ${t.tally.covered} covering servers (verified @r${t.round})` : `none constructible at r${t.round}`}</li>`;
  }
  steps.innerHTML = html;
}

const short = h => String(h).slice(0, 8);

// The same steps with the engine's hashes: leaf, each sibling, each result.
function hashSteps(pos, proof, levels, ent) {
  let idx = pos - proof.peak.start - 1;
  let html = `<li>hash the entry at position ${pos}${ent ? ` (${escapeHtml(entryLabel(ent).title)})` : ''} → <span class="mono">${short(levels[0][idx])}</span></li>`;
  proof.levels.forEach((l, i) => {
    const sib = levels[i][idx ^ 1];
    idx >>= 1;
    html += `<li>combine with the sibling on the ${l.siblingOnRight ? 'right' : 'left'} covering ${range(l.sibling)}, <span class="mono">${short(sib)}</span> → <span class="mono">${short(levels[i + 1][idx])}</span></li>`;
  });
  const top = levels[levels.length - 1][0];
  html += top === proof.peak.root_hex
    ? `<li>equals this tree's root <span class="mono">${short(top)}</span> in the forest</li>`
    : `<li class="bdBad">⚠ recomputed root ${short(top)} ≠ forest root ${short(proof.peak.root_hex)}</li>`;
  return html;
}

function fullTreeControl(peak) {
  const leaves = 2 ** peak.height;
  const tree = trees.get(treeKey(peak));
  if (!(S.smr.live || S.smr.handle)) return '';
  if (leaves > FULL_TREE_MAX_LEAVES) return `<div class="bdDim">tree of ${leaves} entries: too large to draw in full (limit ${FULL_TREE_MAX_LEAVES})</div>`;
  if (tree?.loading) return '<div class="bdDim">recomputing the tree…</div>';
  if (tree?.error) return `<div class="bdDim">full tree unavailable: ${escapeHtml(tree.error)}</div>`;
  if (tree?.levels) return '';
  return `<button class="btn" data-tree="${treeKey(peak)}" data-tip="the engine recomputes every node hash of this tree from the committed sequence">render full tree</button>`;
}

function proofSvg(pos, proof, peaks) {
  const { peak } = proof;
  const leaves = 2 ** peak.height;
  const levels = trees.get(treeKey(peak))?.levels ?? null;
  const skyline = `<div class="bdSky">${peaks.map(pk => `<span class="bdSkyBar${pk === peak ? ' on' : ''}" style="width:${Math.max(8, Math.min(120, 2 ** pk.height * 2))}px" data-tip="tree of ${2 ** pk.height} leaves, positions ${range([pk.start + 1, pk.start + 2 ** pk.height])}">${pk.height}</span>`).join('')}</div>`;
  if (levels) return skyline + wholeTree(pos, proof, levels);
  if (leaves <= WHOLE_TREE_LEAVES) return skyline + wholeTree(pos, proof, null) + fullTreeControl(peak);
  return skyline + pathOnly(proof) + fullTreeControl(peak);
}

function wholeTree(pos, proof, levels) {
  const { peak } = proof;
  const leaves = 2 ** peak.height;
  const DX = leaves <= 64 ? 24 : leaves <= 512 ? 10 : 5, DY = 30, W = leaves * DX + 24, H = (peak.height + 1) * DY + 24;
  const R = Math.min(6, DX / 2 - 0.5);
  const onPath = new Set(proof.levels.map((l, i) => `${i}:${l.path[0]}`).concat([`${peak.height}:${peak.start + 1}`]));
  const sib = new Set(proof.levels.map((l, i) => `${i}:${l.sibling[0]}`));
  const xAt = (lo, hi) => 12 + ((lo + hi - 2 * peak.start - 2) / 2) * DX + DX / 2;
  const yAt = lvl => H - 12 - lvl * DY;
  let edges = '', nodes = '';
  const walk = (lo, hi, lvl) => {
    const x = xAt(lo, hi), y = yAt(lvl);
    const path = onPath.has(`${lvl}:${lo}`) && lo <= pos && pos <= hi;
    const isSib = sib.has(`${lvl}:${lo}`) && !(lo <= pos && pos <= hi);
    const tip = levels ? ` data-tip="${range([lo, hi])} · ${levels[lvl][(lo - peak.start - 1) >> lvl]}"` : '';
    nodes += `<circle${tip} cx="${x}" cy="${y}" r="${lvl === 0 ? R : Math.min(5, R)}" fill="${path ? '#0f1216' : '#1c222a'}" stroke="${path ? TEAL : isSib ? AMBER : '#3a4450'}" stroke-width="${path || isSib ? 2 : 1.2}"/>`;
    if (lvl > 0) {
      const half = (hi - lo + 1) / 2;
      for (const [a, b] of [[lo, lo + half - 1], [lo + half, hi]]) {
        const cx = xAt(a, b), cy = yAt(lvl - 1);
        const hot = path && a <= pos && pos <= b;
        edges += `<line x1="${x}" y1="${y}" x2="${cx}" y2="${cy}" stroke="${hot ? TEAL : '#3a4450'}" stroke-width="${hot ? 2 : 1}"/>`;
        walk(a, b, lvl - 1);
      }
    }
  };
  walk(peak.start + 1, peak.start + leaves, peak.height);
  const note = levels ? `<div class="bdDim">tree of ${leaves} entries, every node hash recomputed by the engine · hover a node for its hash</div>` : '';
  return `<svg width="${W}" height="${H}">${edges}${nodes}</svg>${note}`;
}

function pathOnly(proof) {
  const { peak, levels } = proof;
  const DY = 34, H = (levels.length + 1) * DY + 20, W = 420;
  let out = '';
  levels.forEach((l, i) => {
    const y = H - 18 - i * DY;
    out += `<line x1="60" y1="${y}" x2="60" y2="${y - DY}" stroke="${TEAL}" stroke-width="2"/>` +
      `<line x1="60" y1="${y}" x2="150" y2="${y}" stroke="${AMBER}" stroke-width="1.2" stroke-dasharray="3 3"/>` +
      `<circle cx="60" cy="${y}" r="5" fill="#0f1216" stroke="${TEAL}" stroke-width="2"/>` +
      `<circle cx="150" cy="${y}" r="5" fill="#1c222a" stroke="${AMBER}" stroke-width="2"/>` +
      `<text x="162" y="${y + 4}" fill="#aeb6c0" font-size="11" font-family="ui-monospace, Menlo, monospace">${range(l.sibling)} · ${count(l.sibling)}</text>`;
  });
  const ry = H - 18 - levels.length * DY;
  out += `<circle cx="60" cy="${ry}" r="6" fill="#0f1216" stroke="${TEAL}" stroke-width="2"/>` +
    `<text x="74" y="${ry + 4}" fill="${TEAL}" font-size="11" font-family="ui-monospace, Menlo, monospace">root ${String(peak.root_hex).slice(0, 8)} · ${2 ** peak.height} leaves</text>`;
  return `<svg width="${W}" height="${H}">${out}</svg><div class="bdDim">tree of ${2 ** peak.height} entries: path only</div>`;
}

/* ---------- lifelines: one row per committed entry, ordering vs. its deadline ---------- */

const RED = '#f7768e', GREY = '#6f7986', WAIT = '#3a4450', TICK = '#e6edf3';
const CLS_COLOR = { ok: TEAL, tight: AMBER, aged: RED, unknown: GREY };
const CLS_LABEL = { ok: 'comfortable', tight: 'tight', aged: 'aged before fixed', unknown: 'unknown' };
const clsLabel = cls => CLS_LABEL[cls];
const ROW_H = 6, LL_PAD_L = 58, LL_PAD_R = 14, LL_TOP = 18, LL_AXIS = 6;
const MONO = 'ui-monospace, Menlo, monospace';
const sgn = v => (v < 0 ? '−' : '+') + Math.abs(v) + 'r';
const cmdName = cmd => nameOf(cmd, isPool(S.smr.ctx?.spec));

const llHint = proto => 'Margin = deadline − position fixed.\nBelow 0: aged before fixed · below T/4: tight.' + (proto === 'recovery'
  ? '\nHover: slack = earliest checkpoint that could freeze it (first window end at or after the deadline) − position fixed.'
  : '');

function llRows(rep, c, block, fv) {
  const ll = lifelines(rep.commands, fv.entries, c.T, c.proto);
  const last = Math.min(block.from + block.size, ll.rows.length);
  const shown = Math.min(last, block.from + MAX_CHIPS);
  return { ll, rows: ll.rows.slice(block.from, shown), hidden: last - shown };
}

function renderLifelines(rep, c, block, fv) {
  const main = $('llMain'), side = $('llSide');
  if (!block.size || !fv || !fv.entries) {
    main.innerHTML = `<div class="bdDim">${!block.size ? 'nothing committed in this block' : 'lifelines need the live committed-sequence feed'}</div>`;
    side.innerHTML = '';
    return;
  }
  const { ll, rows, hidden } = llRows(rep, c, block, fv);
  const cmds = rows.filter(r => r.cmd >= 0);
  // the highlighted row is the lab's selected command; the card falls back to the smallest margin
  const selPos = cmds.find(r => r.cmd === S.smr.sel)?.pos ?? null;
  const focus = selPos ?? defaultRow(cmds, c.proto);
  const rounds = rep.metrics.length;
  const span = llSpan(c, block, rows, rounds);
  const W = Math.max(420, main.clientWidth || 760);
  const X = r => LL_PAD_L + ((r - span[0]) / (span[1] - span[0])) * (W - LL_PAD_L - LL_PAD_R);
  const skipped = rows.length ? rows[rows.length - 1].skipped : 0;
  main.innerHTML =
    `<div class="llLegend"><span><i style="background:${TEAL}"></i>delivered → position fixed</span>` +
    `<span><i style="background:${WAIT}"></i>${c.proto === 'recovery' ? 'waiting to execute' : 'waiting to commit'}</span>` +
    `<span><i class="tick"></i>deadline</span>` +
    `<span class="qHint" title="${escapeHtml('Deadline: the prefix through the entry is T rounds old; a bracket marks resends.' +
      (skipped ? `\n${skipped} entr${skipped === 1 ? 'y' : 'ies'} without a stamp: deadlines are lower bounds.` : ''))}">?</span></div>` +
    `<div class="llScroll">${llSvg(rep, c, block, rows, rounds, X, span, W, selPos)}</div>` +
    (hidden ? `<div class="bdDim">+ ${hidden} more entries not drawn</div>` : '');
  main.querySelector('svg.llChart').addEventListener('click', e => {
    const hit = e.target.closest('[data-pos]');
    const r = hit && rows.find(x => x.pos === +hit.dataset.pos);
    if (!r || r.cmd < 0) return;
    selectCmd(r.cmd === S.smr.sel ? null : r.cmd);
  });
  side.innerHTML = summaryCard(c, cmds, ll) + detailCard(rep, c, rows, focus);
  side.querySelector('[data-act=next]')?.addEventListener('click', () => {
    const tight = cmds.filter(r => ['tight', 'aged'].includes(shownMargin(r, c.proto).cls));
    if (tight.length) selectCmd((tight.find(r => r.pos > focus) || tight[0]).cmd);
  });
}

function defaultRow(cmds, proto) {
  const m = r => shownMargin(r, proto).value;
  const known = cmds.filter(r => m(r) !== null);
  if (known.length) return known.reduce((a, b) => (m(b) < m(a) ? b : a)).pos;
  return cmds.length ? cmds[0].pos : null;
}

// Rounds in view: every drawn segment, plus the block's own boundaries.
function llSpan(c, block, rows, rounds) {
  const xs = [];
  for (const r of rows) {
    for (const v of [r.delivered, r.stamp, r.fixed, r.end, r.dlLo, r.dlHi]) if (v !== null && v !== undefined) xs.push(v);
    if (r.cmd >= 0 && (r.fixed === null || r.end === null)) xs.push(rounds);
  }
  if (c.proto === 'recovery') xs.push(block.round - c.T, block.round);
  else xs.push(block.round - c.T + 1, block.round);
  return [Math.max(0, Math.min(...xs) - 2), Math.max(...xs) + 2];
}

function llSvg(rep, c, block, rows, rounds, X, span, W, selPos) {
  const H = LL_TOP + rows.length * ROW_H + LL_AXIS;
  const y = i => LL_TOP + i * ROW_H + ROW_H / 2;
  let grid = '', seg = '', wait = '', ticks = '', brackets = '', hits = '', sel = '', top = '';
  if (c.proto === 'recovery') {
    const freeze = block.round - c.T;
    for (let b = Math.ceil(span[0] / c.T) * c.T; b <= span[1]; b += c.T) {
      const own = b === block.round, frz = b === freeze;
      grid += `<line x1="${X(b)}" y1="${LL_TOP - 4}" x2="${X(b)}" y2="${H - LL_AXIS}" stroke="${own ? TEAL : frz ? '#8b95a1' : '#2c343e'}" stroke-width="${own ? 2 : 1}"${own ? '' : ' stroke-dasharray="3 3"'}/>`;
      const label = own ? `executes r${b}` : frz ? `P frozen r${b}` : `r${b}`;
      const tip = own ? "the boundary that executes this block's P"
        : frz ? "the boundary that froze this block's entries into P: each had to reach its deadline by here" : 'checkpoint boundary';
      grid += `<text x="${own ? X(b) - 4 : X(b) + 4}" y="11"${own ? ' text-anchor="end"' : ''} fill="${own ? TEAL : '#8b95a1'}" font-size="10" font-family="${MONO}" data-tip="${tip}">${label}</text>`;
    }
  } else {
    const lo = block.round - c.T + 1;
    grid += `<rect x="${X(lo)}" y="${LL_TOP - 4}" width="${Math.max(1, X(block.round) - X(lo))}" height="${H - LL_AXIS - LL_TOP + 4}" fill="${TEAL}" fill-opacity="0.05"/>` +
      `<text x="${X(lo)}" y="11" fill="#8b95a1" font-size="10" font-family="${MONO}" data-tip="the display window this block covers: commands committed in it">r${lo}</text>` +
      `<text x="${X(block.round)}" y="11" text-anchor="end" fill="${TEAL}" font-size="10" font-family="${MONO}">r${block.round}</text>`;
  }
  rows.forEach((r, i) => {
    const yy = y(i);
    if (r.pos === selPos) {
      sel = `<rect x="${LL_PAD_L - 4}" y="${yy - ROW_H / 2}" width="${W - LL_PAD_L - LL_PAD_R + 8}" height="${ROW_H}" fill="#e6edf3" fill-opacity="0.08"/>`;
      if (r.delivered !== null) {
        const fixedAt = r.fixed ?? rounds;
        top = `<path d="M${X(r.delivered)},${yy}H${Math.max(X(fixedAt), X(r.delivered) + 1)}" stroke="#ffffff" stroke-width="3.4"/>` +
          (r.fixed !== null ? `<path d="M${X(r.fixed)},${yy}H${X(r.end ?? rounds)}" stroke="#ffffff" stroke-opacity="0.55" stroke-width="3"/>` : '');
      }
    }
    if (r.cmd >= 0 && r.delivered !== null) {
      const fixedAt = r.fixed ?? rounds;
      seg += `<path d="M${X(r.delivered)},${yy}H${Math.max(X(fixedAt), X(r.delivered) + 1)}" stroke="${CLS_COLOR[shownMargin(r, c.proto).cls]}"/>`;
      if (r.fixed !== null) wait += `M${X(r.fixed)},${yy}H${X(r.end ?? rounds)}`;
    } else if (r.kind === 'nop' && r.stamp !== null) {
      seg += `<circle cx="${X(r.stamp)}" cy="${yy}" r="1.6" fill="${GREY}"/>`;
    }
    if (r.dlLo !== null) {
      ticks += `M${X(r.dlLo)},${yy - ROW_H / 2 + 0.5}V${yy + ROW_H / 2 - 0.5}`;
      if (r.dlHi > r.dlLo) brackets += `M${X(r.dlLo)},${yy - ROW_H / 2 + 0.5}H${X(r.dlHi)}`;
    }
    hits += `<rect data-pos="${r.pos}" x="${LL_PAD_L}" y="${yy - ROW_H / 2}" width="${W - LL_PAD_L - LL_PAD_R}" height="${ROW_H}" fill="transparent" data-tip="${escapeHtml(rowTip(rep, c, r))}"/>`;
  });
  const first = rows[0].pos, last = rows[rows.length - 1].pos;
  const labels = `<text x="${LL_PAD_L - 6}" y="${y(0) + 3}" text-anchor="end" fill="#8b95a1" font-size="10" font-family="${MONO}">#${first}</text>` +
    (rows.length > 1 ? `<text x="${LL_PAD_L - 6}" y="${y(rows.length - 1) + 3}" text-anchor="end" fill="#8b95a1" font-size="10" font-family="${MONO}">#${last}</text>` : '');
  return `<svg class="llChart" width="${W}" height="${H}">${grid}${sel}` +
    `<path d="${wait}" stroke="${WAIT}" stroke-width="3" fill="none"/>` +
    `<g stroke-width="3" fill="none">${seg}</g>` +
    `<path d="${brackets}" stroke="#c9d1d9" stroke-opacity="0.45" stroke-width="1" fill="none"/>` +
    `<path d="${ticks}" stroke="${TICK}" stroke-width="1.4" fill="none"/><g fill="none">${top}</g>${labels}${hits}</svg>`;
}

// Recovery adds its slack to the checkpoint after the margin; compact the margin alone.
function marginText(r, proto) {
  const m = shownMargin(r, proto);
  if (m.value === null) return 'position not fixed yet';
  return proto === 'recovery' ? `margin ${sgn(m.value)} (slack to checkpoint ${sgn(m.alt)})` : `margin ${sgn(m.value)}`;
}

function rowTip(rep, c, r) {
  if (r.kind === 'null') return `#${r.pos} ⊥ — no known stamp`;
  if (r.kind === 'nop') return `#${r.pos} no-op` + (r.stamp !== null ? ` stamped r${r.stamp} · deadline r${r.dlLo}` : ' — no known stamp');
  if (r.cmd < 0) return `#${r.pos} — not in this report`;
  const cmd = rep.commands[r.cmd];
  return `#${r.pos} ${cmdName(cmd)} · ${clsLabel(shownMargin(r, c.proto).cls, c.proto)} · ${marginText(r, c.proto)}`;
}

function summaryCard(c, cmds, ll) {
  const n = { ok: 0, tight: 0, aged: 0, unknown: 0 };
  for (const r of cmds) n[shownMargin(r, c.proto).cls]++;
  const stat = cls => `<div class="llStat"><b style="color:${CLS_COLOR[cls]}">${n[cls]}</b><span>${clsLabel(cls, c.proto)}</span></div>`;
  const skips = ll.skips.nop + ll.skips.null + ll.skips.cmd;
  const cap = 'COMMIT MARGIN IN THIS BLOCK';
  return `<div class="llCard"><div class="llCap">${cap}<span class="qHint" title="${escapeHtml(llHint(c.proto) + (skips ? `\n${skips} entr${skips === 1 ? 'y' : 'ies'} in the sequence without a stamp.` : ''))}">?</span></div>` +
    `<div class="llStats">${stat('ok')}${stat('tight')}${stat('aged')}${n.unknown ? stat('unknown') : ''}</div>` +
    marginHist(c, cmds) + '</div>';
}

function marginHist(c, cmds) {
  const ms = cmds.map(r => shownMargin(r, c.proto).value).filter(v => v !== null);
  if (!ms.length) return '<div class="bdDim">no fixed position yet</div>';
  const W = 300, H = 92, padB = 16, padX = 10;
  const lo = Math.min(0, ...ms), hi = Math.max(c.T, ...ms);
  const bins = 24, width = Math.max(1, Math.ceil((hi - lo + 1) / bins));
  const counts = new Array(Math.ceil((hi - lo + 1) / width)).fill(0);
  for (const m of ms) counts[Math.floor((m - lo) / width)]++;
  const top = Math.max(...counts);
  const X = v => padX + ((v - lo) / (hi - lo + 1)) * (W - 2 * padX);
  let bars = '';
  counts.forEach((k, i) => {
    if (!k) return;
    const v = lo + i * width, h = Math.max(2, (k / top) * (H - padB - 6));
    const col = v < 0 ? RED : 4 * v < c.T ? AMBER : TEAL;
    bars += `<rect x="${X(v) + 0.5}" y="${H - padB - h}" width="${Math.max(1, X(v + width) - X(v) - 1)}" height="${h}" fill="${col}" data-tip="margin ${v}${width > 1 ? '…' + (v + width - 1) : ''}r: ${k}"/>`;
  });
  const tick = (v, label) => `<text x="${X(v)}" y="${H - 3}" text-anchor="middle" fill="#8b95a1" font-size="9" font-family="${MONO}">${label}</text>`;
  return `<svg class="llHist" width="${W}" height="${H}">${bars}` +
    `<line x1="${X(0)}" y1="2" x2="${X(0)}" y2="${H - padB}" stroke="${RED}" stroke-dasharray="2 2"/>` +
    tick(0, '0') + tick(c.T / 4, 'T/4') + tick(c.T, 'T') + '</svg>';
}

function detailCard(rep, c, rows, focus) {
  const r = rows.find(x => x.pos === focus);
  if (!r || r.cmd < 0) return '';
  const cmd = rep.commands[r.cmd];
  const shown = shownMargin(r, c.proto);
  const col = CLS_COLOR[shown.cls];
  const v = x => (x === null || x === undefined ? '—' : 'r' + x);
  const dl = r.dlHi > r.dlLo ? `r${r.dlLo}–r${r.dlHi}` : v(r.dlLo);
  const line = (k, val, style = '', tip = '') => `<span style="${style}"${tip ? ` data-tip="${escapeHtml(tip)}"` : ''}>${k}</span><span style="${style}">${val}</span>`;
  const word = 'margin';
  const capTip = c.proto === 'recovery' && shown.alt !== null ? ` data-tip="slack to checkpoint ${sgn(shown.alt)}"` : '';
  return `<div class="llCard" style="border-color:${shown.cls === 'aged' ? '#3a2228' : '#232932'}">` +
    `<div class="llCap" style="color:${col}"${capTip}>#${r.pos} · ${escapeHtml(cmdName(cmd))} · ${shown.value === null ? word + ' unknown' : word + ' ' + sgn(shown.value)}</div>` +
    `<div class="llGrid">` +
    line('delivered', v(r.delivered)) +
    line('in every log', v(r.allLogs), '', 'T_B: the first round the command was in every non-⊥ log') +
    line('deadline', dl, 'color:#e6edf3', r.dlHi > r.dlLo ? 'a range: later accepted resends restamped it in some logs' : 'the prefix through it is T rounds old') +
    line('position fixed', v(r.fixed), `color:${col}`, 'T_E: every non-⊥ server’s executed part plus log agrees on the prefix through it') +
    line(c.proto === 'recovery' ? 'executed' : 'committed', v(r.end)) +
    `</div><div class="llBtns"><button class="btn" data-act="next">next tight ▸</button></div></div>`;
}

/* ---------- order: submission rank → commit position ---------- */

const pctOf = x => (x === null ? '–' : Math.round(100 * x) + '%');
const ORD_W = 760, ORD_LX = 70, ORD_RX = ORD_W - 70;
// in-place lines recede so the reorders stand out
const lineColor = d => (d === 0 ? '#3a4450' : dispColor(d));

function renderOrder(rep, c, block, fv) {
  const main = $('ordMain'), side = $('ordSide');
  if (!block.size || !fv || !fv.entries) {
    main.innerHTML = `<div class="bdDim">${!block.size ? 'nothing committed in this block' : 'the order needs the live committed-sequence feed'}</div>`;
    side.innerHTML = '';
    return;
  }
  const runOrder = committedOrder(rep, fv.entries);
  const placed = committedEntries(rep, fv.entries.slice(block.from, block.from + block.size), block.from);
  const order = placed.map(x => x.cmd);
  const disp = displacements(rep, order);
  // left: submission order (injection round; ties kept in commit order so they never cross)
  const items = order.map((ci, k) => ({ ci, k, pos: placed[k].pos, inj: rep.commands[ci].injection_round, d: disp[k] }));
  const bySub = [...items].sort((a, b) => a.inj - b.inj || a.k - b.k);
  bySub.forEach((it, s) => { it.s = s; });
  const H = Math.max(160, Math.min(520, items.length * 5 + 40)), top = 12, bot = H - 14;
  const Y = k => top + (items.length > 1 ? (k / (items.length - 1)) * (bot - top) : (bot - top) / 2);
  let lines = '', hits = '', labels = '';
  const sel = items.find(it => it.ci === S.smr.sel) ?? null;
  for (const it of [...items].sort((a, b) => Math.abs(a.d) - Math.abs(b.d))) {
    if (it === sel) continue;
    lines += `<line x1="${ORD_LX}" y1="${Y(it.s)}" x2="${ORD_RX}" y2="${Y(it.k)}" stroke="${lineColor(it.d)}" stroke-width="${Math.abs(it.d) >= 4 ? 1.6 : 1.1}"/>`;
  }
  if (sel) lines += `<line x1="${ORD_LX}" y1="${Y(sel.s)}" x2="${ORD_RX}" y2="${Y(sel.k)}" stroke="#0f1216" stroke-width="5"/>` +
    `<line x1="${ORD_LX}" y1="${Y(sel.s)}" x2="${ORD_RX}" y2="${Y(sel.k)}" stroke="#ffffff" stroke-width="2.2"/>`;
  for (const it of items) {
    const cmd = rep.commands[it.ci];
    hits += `<line class="hit" data-cmd="${it.ci}" x1="${ORD_LX}" y1="${Y(it.s)}" x2="${ORD_RX}" y2="${Y(it.k)}" stroke="transparent" stroke-width="6"` +
      ` data-tip="#${it.pos} ${escapeHtml(cmdName(cmd))} · submitted r${it.inj} · displacement ${signedInt(it.d)}"/>`;
  }
  let lastLab = -99;
  bySub.forEach((it, s) => {
    if (s && it.inj === bySub[s - 1].inj) return;
    labels += `<line x1="${ORD_LX - 4}" y1="${Y(s)}" x2="${ORD_LX}" y2="${Y(s)}" stroke="#8b95a1"/>`;
    if (Y(s) - lastLab > 13) { labels += `<text x="${ORD_LX - 8}" y="${Y(s) + 3}" text-anchor="end" fill="#8b95a1" font-size="10" font-family="${MONO}">r${it.inj}</text>`; lastLab = Y(s); }
  });
  for (const k of new Set([0, Math.round((items.length - 1) / 2), items.length - 1])) {
    labels += `<text x="${ORD_RX + 8}" y="${Y(k) + 3}" fill="#8b95a1" font-size="10" font-family="${MONO}">#${items[k].pos}</text>`;
  }
  const worst = [...items].sort((a, b) => Math.abs(b.d) - Math.abs(a.d))[0];
  for (const it of new Set([worst, sel].filter(Boolean))) {
    if (it.d === 0 && it !== sel) continue;
    labels += `<text x="${ORD_RX + 8}" y="${Y(it.k) + 15}" fill="#e6edf3" font-size="10" font-family="${MONO}">#${it.pos}: ${signedInt(it.d)}</text>`;
  }
  const legend = [['0', lineColor(0)], ['1–3', dispColor(2)], ['4–9', dispColor(5)], ['10+', dispColor(10)], ['selected', '#ffffff']]
    .map(([l, col]) => `<span><i style="background:${col}"></i>${l}</span>`).join('');
  main.innerHTML = `<div class="llLegend" style="width:${ORD_RX + 50}px"><span>submitted</span><span class="qHint" title="Left: submission order (injection round). Right: commit position. A crossing is a reorder.">?</span><span style="flex:1"></span><span>committed</span></div>` +
    `<svg class="ordChart" width="${ORD_W}" height="${H}">${labels}${lines}${hits}</svg><div class="ordLegend">|displacement| ${legend}</div>`;
  main.querySelector('svg.ordChart').addEventListener('click', e => {
    const hit = e.target.closest('[data-cmd]');
    if (!hit) return;
    const i = +hit.dataset.cmd;
    selectCmd(i === S.smr.sel ? null : i);
  });
  const injs = items.map(it => it.inj);
  const runDisp = displacements(rep, runOrder);
  const f = fifoCheck(rep, runOrder, new Set(order));
  side.innerHTML =
    `<div class="llCard"><div class="llCap">DISPLACEMENT · ${escapeHtml(block.label)}<span class="qHint" title="Commit rank − submission rank within this block; same-round submissions tied.">?</span></div>` +
    histSvg(disp, { W: 290, H: 80, lo: -16, hi: 8, color: dispColor, ticks: [-15, -10, -5, 0, 5], xlab: signedInt, tip: (l, k) => `${l}: ${k}` }) +
    `<div class="ordBig">${pctOf(withinShare(disp, 3))}<span> within ±3 places</span></div>` +
    `<div class="sumStat">${pctOf(withinShare(disp, 0))} in place · range ${signedInt(Math.min(...disp))} to ${signedInt(Math.max(...disp))}</div>` +
    `<div class="sumStat">submitted r${Math.min(...injs)}–r${Math.max(...injs)} · ${new Set(injs).size} rounds</div></div>` +
    `<div class="llCard"><div class="llCap">PER-CLIENT FIFO<span class="qHint" title="Per client, issue order = commit order.">?</span></div>` +
    `<div class="sumStat">${f.status === 'na' ? (f.multi ? 'not applicable · no client has two commands to compare here' : 'not applicable · every client issued one command') : f.status === 'ok' ? `holds · ${f.pairs} pairs` : `${f.violations} out of order`}</div></div>` +
    `<div class="llCard"><div class="llCap">RUN</div><div class="sumStat">${pctOf(withinShare(runDisp, 3))} within ±3 · ${pctOf(withinShare(runDisp, 0))} in place</div></div>`;
}

/* ---------- spread drawer ---------- */

function measuredSet(rep) {
  const v = $('drMeasure').value;
  const observed = rep.commands.filter(cmd => cmd.spread && cmd.spread.length && cmd.delivered_round !== null);
  if (v === 'view') {
    const r = viewRange();
    return r ? observed.filter(cmd => cmd.delivered_round >= r[0] && cmd.delivered_round <= r[1]) : observed;
  }
  return v === 'all' ? observed : observed.slice(-(+v));
}

function renderSpread(rep, c) {
  $('drawerKind').textContent = 'Spread';
  $('drawerTitle').textContent = '';
  $('drawerWarn').classList.add('hidden');
  setTabs([['dist', 'Distribution'], ['pos', 'Position']], d.tab, k => { d.tab = k; render(); });
  $('drMeasure').classList.toggle('hidden', d.tab !== 'dist');
  const P = prepCanvas($('drSpread'));
  if (!P) return;
  if (d.tab === 'pos') { drawPosition(rep, P); return; }
  drawDistribution(rep, c, P);
}

function prepCanvas(cv) {
  const dpr = window.devicePixelRatio || 1;
  const W = cv.clientWidth, H = 280;
  if (W < 2) return null;
  cv.style.height = H + 'px';
  cv.width = W * dpr; cv.height = H * dpr;
  const g = cv.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, W, H);
  g.font = '10px ui-monospace, Menlo, monospace';
  return { g, W, H };
}

function drawDistribution(rep, c, { g, W, H }) {
  const set = measuredSet(rep);
  const st = spreadStats(set, c.n);
  $('drHead').textContent = st.measured
    ? `${set.length} commands · median ${st.median}r · p90 ${st.p90}r to every server · ${st.multiple.toFixed(2)}× log₂n` + (st.spreading ? ` · ${st.spreading} still spreading` : '')
    : `${set.length} commands · none has reached every server yet` + (st.spreading ? ` (${st.spreading} still spreading)` : '');
  const padL = 44, padR = 16, padT = 14, padB = 26;
  const curves = set.map(cmd => spreadCurve(cmd).map(p => ({ x: p.round - cmd.delivered_round, y: p.useful_total ? p.useful_holders / p.useful_total : 0 })).filter(p => p.x >= 0));
  // the curves keep going after a command commits, so the axis ends a little
  // past the slowest measured spread instead of at the longest curve
  const slowest = Math.max(0, ...set.filter(cmd => cmd.all_logs_round !== null).map(cmd => cmd.all_logs_round - cmd.delivered_round));
  const xmax = Math.max(4, Math.ceil(1.6 * Math.max(slowest, c.n > 1 ? Math.log2(c.n) : 0)));
  const X = x => padL + (x / xmax) * (W - padL - padR);
  const Y = y => H - padB - y * (H - padT - padB);
  g.strokeStyle = '#1c2129'; g.fillStyle = '#565f6b'; g.textAlign = 'right'; g.textBaseline = 'middle'; g.lineWidth = 1;
  for (const y of [0, 0.5, 1]) { g.beginPath(); g.moveTo(padL, Y(y)); g.lineTo(W - padR, Y(y)); g.stroke(); g.fillText(y.toFixed(1), padL - 6, Y(y)); }
  g.textAlign = 'center'; g.textBaseline = 'top';
  const step = Math.max(1, Math.ceil(xmax / 10));
  for (let x = 0; x <= xmax; x += step) g.fillText(String(x), X(x), H - padB + 6);
  g.textAlign = 'left'; g.fillText('rounds since the command was delivered', padL, H - 12);
  g.strokeStyle = 'rgba(61,214,196,0.18)'; g.lineWidth = 1;
  for (const cv of curves) {
    g.beginPath();
    cv.filter(p => p.x <= xmax).forEach((p, i) => (i ? g.lineTo(X(p.x), Y(p.y)) : g.moveTo(X(p.x), Y(p.y))));
    g.stroke();
  }
  // median and 10–90 % band over the commands that have a sample at that age
  const med = [], lo = [], hi = [];
  for (let x = 0; x <= xmax; x++) {
    const vals = curves.map(cv => cv.find(p => p.x === x)).filter(Boolean).map(p => p.y).sort((a, b) => a - b);
    if (vals.length < 3) continue;
    const q = f => vals[Math.min(vals.length - 1, Math.floor(f * vals.length))];
    med.push([x, q(0.5)]); lo.push([x, q(0.1)]); hi.push([x, q(0.9)]);
  }
  if (med.length > 1) {
    g.fillStyle = 'rgba(238,242,246,0.12)';
    g.beginPath(); hi.forEach(([x, y], i) => (i ? g.lineTo(X(x), Y(y)) : g.moveTo(X(x), Y(y))));
    for (let i = lo.length - 1; i >= 0; i--) g.lineTo(X(lo[i][0]), Y(lo[i][1]));
    g.closePath(); g.fill();
    g.strokeStyle = '#eef2f6'; g.lineWidth = 2;
    g.beginPath(); med.forEach(([x, y], i) => (i ? g.lineTo(X(x), Y(y)) : g.moveTo(X(x), Y(y)))); g.stroke();
  }
  if (c.n > 1) {
    const lx = X(Math.log2(c.n));
    g.strokeStyle = AMBER; g.lineWidth = 1; g.setLineDash([4, 4]);
    g.beginPath(); g.moveTo(lx, padT); g.lineTo(lx, H - padB); g.stroke(); g.setLineDash([]);
    g.fillStyle = AMBER; g.textAlign = 'left'; g.textBaseline = 'top';
    g.fillText(`log₂ n = ${Math.log2(c.n).toFixed(1)} (scale reference only: the paper proves O(log n), no constant)`, lx + 5, padT);
  }
  g.fillStyle = '#6f7986'; g.textAlign = 'right'; g.textBaseline = 'top';
  g.fillText('white: median · band: 10–90 % · each curve ends one round after every log has it', W - padR, padT);
}

// Position tab: one command's min / median / max log position per round; the
// band collapses at T_E, the round its prefix stops moving.
function drawPosition(rep, { g, W, H }) {
  $('drHead').textContent = '';
  const sel = S.smr.sel !== null && S.smr.sel < rep.commands.length ? S.smr.sel : rep.commands.length - 1;
  if (sel < 0) { g.fillStyle = '#565f6b'; g.fillText('no commands yet', 52, 24); return; }
  const cmd = rep.commands[sel];
  const label = cmdName(cmd);
  $('drHead').textContent = `${label} — pick another in the audit trail`;
  const pts = (cmd.spread || []).filter(p => p.pos_max !== undefined && p.pos_max !== null);
  const padL = 52, padR = 16, padT = 14, padB = 26;
  if (!pts.length) { g.fillStyle = '#565f6b'; g.fillText('no position samples yet — the command is not in any log', padL, padT + 10); return; }
  const r0 = pts[0].round, r1 = Math.max(pts[pts.length - 1].round, r0 + 1);
  const maxPos = Math.max(...pts.map(p => p.pos_max), 1);
  const X = r => padL + ((r - r0) / (r1 - r0)) * (W - padL - padR);
  const Y = p => H - padB - (p / maxPos) * (H - padT - padB - 8);
  g.strokeStyle = '#1c2129'; g.fillStyle = '#565f6b'; g.textAlign = 'right'; g.textBaseline = 'middle';
  for (const f of [0, 0.5, 1]) { const p = Math.round(maxPos * f); g.beginPath(); g.moveTo(padL, Y(p)); g.lineTo(W - padR, Y(p)); g.stroke(); g.fillText(String(p), padL - 6, Y(p)); }
  g.textAlign = 'center'; g.textBaseline = 'top';
  g.fillText('r' + r0, X(r0), H - padB + 6); g.fillText('r' + r1, X(r1), H - padB + 6);
  g.fillStyle = 'rgba(61,214,196,0.18)';
  g.beginPath();
  pts.forEach((p, j) => (j ? g.lineTo(X(p.round), Y(p.pos_max)) : g.moveTo(X(p.round), Y(p.pos_max))));
  for (let j = pts.length - 1; j >= 0; j--) g.lineTo(X(pts[j].round), Y(pts[j].pos_min));
  g.closePath(); g.fill();
  g.strokeStyle = TEAL; g.lineWidth = 2.2;
  g.beginPath(); pts.forEach((p, j) => (j ? g.lineTo(X(p.round), Y(p.pos_med)) : g.moveTo(X(p.round), Y(p.pos_med)))); g.stroke();
  if (cmd.prefix_fixed_round !== null) {
    const x = X(cmd.prefix_fixed_round);
    g.strokeStyle = '#bb9af7'; g.lineWidth = 1.4;
    g.beginPath(); g.moveTo(x, padT); g.lineTo(x, H - padB); g.stroke();
    g.fillStyle = '#bb9af7'; g.textAlign = 'center'; g.fillText('T_E', x, H - padB + 6);
  }
}
