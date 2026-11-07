// Drawers under the timeline, at most one open: the block drawer (one chain
// block: its entries, the proof path of one entry through the Merkle forest,
// the whole forest, certificates) and the spread drawer (how fast commands
// reach every server, and where a command sits in the logs). Everything shown
// comes from the report or from the engine's forest feed; the forest feed
// carries peaks and roots only, so the proof path draws structure and never a
// hash value it was not given.
import { S, $, PALETTE } from './state.js';
import { forestView, certsLoading, entryLabel, tallyFor, cmdIndexFor } from './smr-certs.js';
import { chainBlocks, spreadStats } from './smr-derive.js';
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
});

// smr.js asks the engine for the forest feed when the block drawer opens: steps skip it while closed.
let onBlockDrawerOpen = () => {};
export function useBlockDrawerOpen(fn) { onBlockDrawerOpen = fn; }
export const blockDrawerOpen = () => d.open === 'block';

function openBlock(block) {
  const wasOpen = blockDrawerOpen();
  d.open = 'block';
  d.block = block;
  d.entry = null;
  d.view = 'this';
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
export function closeDrawer() {
  d.open = null;
  S.smr.blockSel = null;
  render();
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
  setTabs([['this', 'This block'], ['forest', 'Whole forest']], d.view, k => { d.view = k; render(); });
  $('bdThis').classList.toggle('hidden', d.view !== 'this');
  $('bdForest').classList.toggle('hidden', d.view !== 'forest');
  const noForest = !fv;
  $('bdNoForest').classList.toggle('hidden', !noForest);
  $('bdNoForest').textContent = certsLoading()
    ? 'loading the committed sequence…'
    : 'No committed-sequence feed for this run: a spec (batch) run records the report only. Open a live session to see the forest and its certificates.';
  renderWarn(c, fv);
  if (d.view === 'this') { renderChips(rep, c, block, fv); renderProof(rep, c, fv); }
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

function entryChipColor(ent) {
  if (ent.kind !== 'cmd') return '#6f7986';
  const i = cmdIndexFor(ent);
  return i < 0 ? '#c9d2dd' : PALETTE[i % PALETTE.length];
}

function renderChips(rep, c, block, fv) {
  const box = $('bdChips');
  if (!block.size) {
    box.innerHTML = '<span class="bdDim">nothing committed in this block' +
      (block.state === 'rollback' ? ': the boundary rolled servers back instead' : '') + '</span>';
    return;
  }
  const hasEntries = fv && fv.entries;
  if (!hasEntries) {
    // no forest feed: only the report's client commands are known, not positions
    box.innerHTML = `<span class="bdDim">entry positions need the live forest feed; client commands in this block:</span>` +
      (block.cmds.length ? block.cmds.map(i => {
        const cmd = rep.commands[i];
        return `<span class="bdChip mono" style="color:${PALETTE[i % PALETTE.length]}">${cmd.auto ? 'bg #' + (cmd.client - 2147483648) : `c${cmd.client}·op${cmd.op}`}</span>`;
      }).join('') : '<span class="bdDim">none</span>');
    return;
  }
  const last = Math.min(block.from + block.size, fv.entries.length);
  const shown = Math.min(last, block.from + MAX_CHIPS);
  let html = '';
  for (let p = block.from + 1; p <= shown; p++) {
    const ent = fv.entries[p - 1];
    const lab = entryLabel(ent);
    html += `<button class="bdChip mono${d.entry === p ? ' on' : ''}" data-pos="${p}" style="color:${entryChipColor(ent)}" data-tip="${escapeHtml(lab.title)} — position ${p}">#${p} ${escapeHtml(lab.text)}</button>`;
  }
  if (last > shown) html += `<span class="bdDim">+ ${last - shown} more entries</span>`;
  box.innerHTML = html;
  for (const b of box.querySelectorAll('.bdChip[data-pos]')) {
    b.addEventListener('click', () => {
      const p = +b.dataset.pos;
      d.entry = d.entry === p ? null : p;
      const ent = fv.entries[p - 1];
      if (d.entry !== null && ent.kind === 'cmd') {
        const i = cmdIndexFor(ent);
        if (i >= 0) S.smr.sel = i;
      }
      render();
    });
  }
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

function renderProof(rep, c, fv) {
  const viz = $('bdProofViz'), steps = $('bdSteps');
  if (!fv || d.entry === null) {
    viz.innerHTML = '<div class="bdDim">' + (fv && fv.entries ? 'pick an entry above to see its path through the Merkle forest' : '') + '</div>';
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
    html += `<li>certificate: ${t === null ? 'press verify everywhere to check it against every server' : t.tally ? `accepted by ${t.tally.accepted} of ${t.tally.covered} covering servers (verified @r${t.round})` : `none constructible at r${t.round}`}</li>`;
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
  const curves = set.map(cmd => cmd.spread.map(p => ({ x: p.round - cmd.delivered_round, y: p.useful_total ? p.useful_holders / p.useful_total : 0 })).filter(p => p.x >= 0));
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
  g.fillText('white: median across commands · band: 10–90 %', W - padR, padT);
}

// Position tab: one command's min / median / max log position per round; the
// band collapses at T_E, the round its prefix stops moving.
function drawPosition(rep, { g, W, H }) {
  $('drHead').textContent = '';
  const sel = S.smr.sel !== null && S.smr.sel < rep.commands.length ? S.smr.sel : rep.commands.length - 1;
  if (sel < 0) { g.fillStyle = '#565f6b'; g.fillText('no commands yet', 52, 24); return; }
  const cmd = rep.commands[sel];
  const label = cmd.auto ? `bg #${cmd.client - 2147483648}` : `c${cmd.client} · op ${cmd.op}`;
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
