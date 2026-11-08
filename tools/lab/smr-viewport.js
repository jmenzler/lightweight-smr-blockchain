// Viewport timeline: one shared x-axis of rounds, a fixed span that scrolls
// sideways, an overview strip of the whole run, a checks indicator, and lanes for
// the servers, resets and rollbacks, command spread, oldest unsettled age, backlog
// by stage and the committed blocks. Canvas, because a long run redraws every
// round. Every plotted value comes from the report via smr-derive.js; the lane
// code only maps it to pixels. Full lane definitions live in the about panel.
import { S, $ } from './state.js';
import {
  derive, programBeta, chainBlocks, timelineEvents, phaseBands, spreadStats, neverCommitted, recRows,
  stageSeries, safetyChecks, serverAxis, rStarScan, releaseRound, lifelines, blockMinMargin,
  cmdHighlight, nextSelection, blockLife, shownMargin, endText,
  checksLabel, displacements, withinShare, committedOrder, fifoCheck, inViewSet, spreadCurve, stageLatencies,
  formatBytes, memoryLevel, rollbackRounds, staleNote, committedEntries,
} from './smr-derive.js';
import { forestView } from './smr-certs.js';
import { histSvg, dispColor, signedInt } from './smr-hist.js';
import { showTip, hideTip, escapeHtml } from './tooltip.js';

const PADL = 150, PADR = 50;
const MIN_SPAN = 100, EXTENDED_SPAN = 200, WINDOWS_IN_SPAN = 11;
const C = {
  panel: '#12161b', grid: '#1c222a', bgrid: '#1f262e', t1: '#c9d1d9', t2: '#aeb6c0', t3: '#8b95a1', hi: '#e6edf3', mute: '#565f6b',
  teal: '#3dd6c4', tint: '#5fe8d6', amber: '#e0af68', red: '#f7768e', blue: '#7aa2f7', purple: '#a99cf0', grey: '#4b5563',
  guide: '#9aa3ae', age: '#d7dde4', faint: '#5f6975',
};
const MONO = 'ui-monospace, Menlo, monospace', SANS = 'ui-sans-serif, system-ui, sans-serif';
const font = (px, weight = 400, fam = MONO) => `${weight} ${px}px ${fam}`;

const vp = { follow: true, lo: 0, hover: null, pinned: null, hits: [], layout: null, drag: null, rep: null, hoverBlock: null };
const handlers = { openBlock: () => {}, openSpread: () => {}, selectCmd: () => {} };
export function useViewportHandlers(h) { Object.assign(handlers, h); }

export function resetViewport() {
  Object.assign(vp, { follow: true, lo: 0, hover: null, pinned: null, hits: [], drag: null, hoverBlock: null });
}

const ctxOf = () => S.smr.ctx;
const spanOf = c => (c.proto === 'extended' || !c.T ? EXTENDED_SPAN : Math.max(WINDOWS_IN_SPAN * c.T, MIN_SPAN));
// position fixed (T_E) is recorded by live sessions only
const fixedLive = c => c.kind === 'live';

// A run shorter than the span fills the width; a longer one shows exactly one
// span, following the newest round until the user pans away.
// A live session with a blocking program shows a quarter span of the plan
// ahead of the newest round, so a coming surge is visible before it lands.
function lookahead(span) {
  return S.smr.live && S.smr.program ? Math.round(span / 4) : 0;
}
function domain(total, span) {
  const end = total + lookahead(span);
  if (end <= span) return [0, Math.max(end, 10)];
  const lo = vp.follow ? end - span : Math.min(Math.max(vp.lo, 0), end - span);
  return [lo, lo + span];
}
export function viewRange() {
  const rep = vp.rep, c = ctxOf();
  if (!rep || !c) return null;
  return domain(rep.metrics.length, spanOf(c));
}

function prep(cv, h) {
  const dpr = window.devicePixelRatio || 1;
  const W = cv.clientWidth;
  if (W < 2) return null;
  cv.style.height = h + 'px';
  cv.width = Math.round(W * dpr); cv.height = Math.round(h * dpr);
  const g = cv.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, W, h);
  return { g, W, H: h };
}

// The engine records the oldest unsettled age in live compact and recovery sessions only.
const agesOf = c => (c.proto !== 'extended' && S.smr.ages && S.smr.ages.length ? S.smr.ages : null);
const hasTiles = c => c.proto === 'recovery' || c.proto === 'compact';

// The bracket row links a block's commands to its tile.
function lanesFor(c) {
  const lanes = [['servers', 104, 3]];
  if (c.proto === 'recovery') lanes.push(['resets', 16, 12]);
  lanes.push(['spread', 84, 12]);
  if (agesOf(c)) lanes.push(['age', 70, 12]);
  lanes.push(['backlog', 150, 1]);
  if (hasTiles(c)) lanes.push(['bracket', 10, 1], ['tiles', 30, 8]);
  else lanes.push(['gap', 0, 6]);
  lanes.push(['legend', 12, 4], ['axis', 20, 0]);
  let y = 2;
  const placed = {};
  for (const [key, h, gap] of lanes) { placed[key] = { y, h }; y += h + gap; }
  return { placed, total: y + 2 };
}

const nice = v => {
  if (v <= 5) return Math.max(1, Math.ceil(v));
  const p = 10 ** Math.floor(Math.log10(v));
  return Math.ceil(v / p) * p;
};

export function renderViewport(rep) {
  vp.rep = rep;
  const c = ctxOf();
  if (!c || !rep.metrics.length) { for (const id of ['vpWrap', 'vpChecks', 'vpCheckCard']) $(id).classList.add('hidden'); return; }
  $('vpWrap').classList.remove('hidden');
  $('vpOverview').dataset.tip = c.proto === 'recovery'
    ? 'Whole run: teal no-reset · amber reset · grey ⊥. Drag to move the view.'
    : 'Whole run: teal holds a log · grey ⊥. Drag to move the view.';
  renderChecks(rep, c);
  drawOverview(rep, c);
  drawLanes(rep, c);
  $('vpLive').classList.toggle('hidden', vp.follow);
  renderReadout(rep, c);
  renderSummary(rep, c);
}

/* ---------- checks: one muted indicator, red chips only for failures ---------- */

const CHECK_MARK = { ok: '✓', fail: '✕', vacuous: '○', unrecorded: '?' };
let checkCardOpen = false;
function renderChecks(rep, c) {
  const box = $('vpChecks');
  const chips = safetyChecks(rep, c.proto, S.smr.safetyRound ?? null);
  box.classList.toggle('hidden', !chips.length);
  const failed = chips.filter(x => x.state === 'fail');
  if (!failed.length) checkCardOpen = false;
  $('vpCheckCard').classList.toggle('hidden', !checkCardOpen);
  if (!chips.length) return;
  const word = { vacuous: 'not applicable, nothing executed yet', unrecorded: 'not recorded in this report', ok: 'passed' };
  const lines = chips.map(x => `${CHECK_MARK[x.state]} ${x.name}: ${x.state === 'fail' ? `failed${x.at ? ' from r' + x.at : ''}` : word[x.state]}`);
  box.innerHTML = failed.length
    ? `<button class="chkSum bad" data-tip="${escapeHtml(lines.join('\n'))}">${checksLabel(chips)} <span class="chkCaret">${checkCardOpen ? '▴' : '▾'}</span></button>`
    : `<span class="chkSum" data-tip="${escapeHtml(lines.join('\n'))}">${checksLabel(chips)}</span>`;
  if (checkCardOpen) $('vpCheckCard').innerHTML = checkCard(chips);
}

// Failed checks with their evidence and a jump; the rest folded into one line each.
function checkCard(chips) {
  const failed = chips.filter(x => x.state === 'fail'), na = chips.filter(x => x.state === 'vacuous'), ok = chips.filter(x => x.state === 'ok');
  const row = (cls, name, at, text, jump) => `<div class="ccRow ${cls}"><i></i><span class="ccName">${name}</span><span class="ccAt">${at}</span><span class="ccText">${escapeHtml(text)}</span>` +
    (jump ? `<button class="vBtn ccJump" data-at="${jump}">jump</button>` : '<span></span>') + '</div>';
  const e = S.smr.end;
  const phase = e?.detail?.phase ? ' · ' + e.detail.phase.replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase() : '';
  const end = !e ? 'run in progress' : e.kind === 'failed' || e.kind === 'aborted' ? `run end · abort r${e.round}${phase} (not a check)` : `run end · ${endText(e)} (not a check)`;
  return failed.map(x => row('fail', x.name, x.at ? 'r' + x.at : '', x.evidence ?? '', x.at)).join('') +
    '<div class="ccSep"></div>' +
    (na.length ? row('na', `${na.length} not applicable`, '', `nothing executed yet: ${na.map(x => x.name).join(', ')}`, null) : '') +
    (chips.some(x => x.state === 'unrecorded') ? row('na', `${chips.filter(x => x.state === 'unrecorded').length} not recorded`, '', 'the report carries no field for: ' + chips.filter(x => x.state === 'unrecorded').map(x => x.name).join(', '), null) : '') +
    row('ok', `${ok.length} passed`, '', ok.length ? ok.map(x => x.name).join(', ') : 'none', null) +
    `<div class="ccSep"></div><div class="ccEnd">${escapeHtml(end)}</div>`;
}
$('vpChecks').addEventListener('click', e => {
  if (!e.target.closest('.chkSum.bad')) return;
  checkCardOpen = !checkCardOpen;
  redraw();
});
$('vpCheckCard').addEventListener('click', e => {
  const b = e.target.closest('.ccJump');
  if (b) jumpTo(+b.dataset.at);
});

// Pin the cursor on a round and pan so it is in view.
function jumpTo(round) {
  const rep = vp.rep, c = ctxOf();
  if (!rep || !c) return;
  const r = Math.max(1, Math.min(rep.metrics.length, round));
  vp.pinned = r;
  const span = spanOf(c), total = rep.metrics.length + lookahead(span);
  const [d0, d1] = domain(rep.metrics.length, span);
  if (total > span && (r < d0 || r > d1)) {
    vp.lo = Math.min(Math.max(r - span / 2, 0), total - span);
    vp.follow = vp.lo >= total - span - 0.5;
  }
  redraw();
}

/* ---------- overview strip ---------- */

function healthShares(rep, c, i) {
  const rows = recRows(rep);
  const n = c.n;
  if (c.proto === 'recovery' && rows) return { a: rows[i].noreset / n, b: rows[i].reset / n, g: rows[i].bot_r / n };
  const m = rep.metrics[i];
  return { a: m.nonbot_logs / n, b: 0, g: (n - m.nonbot_logs) / n };
}

function drawOverview(rep, c) {
  const P = prep($('vpOverview'), 14);
  if (!P) return;
  const { g, W, H } = P;
  const total = rep.metrics.length;
  const x = r => PADL + (r / total) * (W - PADL - PADR);
  const colW = Math.max(1, (W - PADL - PADR) / total);
  g.fillStyle = '#14181e';
  g.fillRect(PADL, 0, W - PADL - PADR, H);
  for (let i = 0; i < total; i++) {
    const s = healthShares(rep, c, i);
    let y = H;
    for (const [v, col] of [[s.a, C.teal], [s.b, C.amber], [s.g, C.grey]]) {
      g.globalAlpha = 0.55; g.fillStyle = col;
      g.fillRect(x(i), y - v * H, colW + 0.5, v * H);
      y -= v * H;
    }
  }
  g.globalAlpha = 1;
  g.fillStyle = C.mute; g.font = font(9.5); g.textAlign = 'left'; g.textBaseline = 'middle';
  g.fillText('whole run (drag)', 8, H / 2);
  const [d0, d1] = domain(total, spanOf(c));
  g.strokeStyle = '#eef2f6'; g.lineWidth = 1.2;
  g.strokeRect(x(d0) + 0.5, 0.5, Math.max(4, x(Math.min(d1, total)) - x(d0)) - 1, H - 1);
  vp.overview = { x, total, W };
}

/* ---------- lanes ---------- */

function drawLanes(rep, c) {
  const { placed, total: height } = lanesFor(c);
  vp.layout = placed;
  const P = prep($('vpCanvas'), height);
  if (!P) return;
  const { g, W } = P;
  const nRounds = rep.metrics.length;
  const [d0, d1] = domain(nRounds, spanOf(c));
  const plotW = W - PADL - PADR;
  const X = r => PADL + ((r - d0) / (d1 - d0)) * plotW;
  const first = Math.max(1, Math.floor(d0));
  const last = Math.min(nRounds, Math.ceil(d1));
  const D = derive(rep, c.proto);
  const blocks = hasTiles(c) ? chainBlocks(rep, c.proto, c.T) : [];
  const sel = S.smr.blockSel != null ? blocks.find(b => b.m === S.smr.blockSel) ?? null : null;
  const life = sel ? blockLife(rep, sel, c.proto, c.T) : null;
  const a = { g, rep, c, X, d0, d1, first, last, D, W, plotW, blocks, sel, life, blockCmds: sel ? new Set(sel.cmds) : null };
  vp.args = a;
  vp.hits = [];
  vp.marks = [];
  hintsUsed.clear();

  const gridBottom = placed.backlog.y + placed.backlog.h;
  if (c.T) {
    g.strokeStyle = C.bgrid; g.lineWidth = 1;
    for (let b = Math.ceil(d0 / c.T) * c.T; b <= d1; b += c.T) {
      g.beginPath(); g.moveTo(X(b) + 0.5, placed.servers.y); g.lineTo(X(b) + 0.5, gridBottom); g.stroke();
    }
  }
  drawServers(a, placed.servers);
  if (placed.resets) drawResets(a, placed.resets);
  drawSpread(a, placed.spread);
  if (placed.age) drawAge(a, placed.age);
  drawBacklog(a, placed.backlog);
  if (placed.tiles) drawTiles(a, placed.tiles);
  if (placed.bracket) drawBrackets(a, placed);
  drawLegend(a, placed.legend);
  drawEventMarkers(a, placed.servers.y, gridBottom);
  drawAxisAndCursor(a, placed);
  for (const [key, el] of hints) el.classList.toggle('hidden', !hintsUsed.has(key));
}

// Lane labels sit in the left column: name, a ? with one short line, the question the lane answers.
const hints = new Map();
const hintsUsed = new Set();
function placeHint(key, x, y, tip) {
  let el = hints.get(key);
  if (!el) {
    el = document.createElement('span');
    el.className = 'qHint vpHint';
    el.textContent = '?';
    $('vpHints').appendChild(el);
    hints.set(key, el);
  }
  el.dataset.tip = tip;
  el.style.left = Math.round(x) + 'px';
  el.style.top = Math.round(y) + 'px';
  hintsUsed.add(key);
}
function laneLabel(g, key, y, name, question, tip, note = null) {
  g.textAlign = 'left'; g.textBaseline = 'alphabetic';
  g.font = font(10, 600); g.fillStyle = C.t3; g.fillText(name, 8, y + 12);
  placeHint(key, 8 + g.measureText(name).width + 6, y + 2, tip);
  g.font = font(11, 400, SANS); g.fillStyle = C.t2; g.fillText(question, 8, y + 27);
  if (note) { g.font = font(9.5); g.fillStyle = C.t3; g.fillText(note, 8, y + 41); }
}
function panel(a, { y, h }) {
  a.g.fillStyle = C.panel; a.g.fillRect(PADL, y, a.plotW, h);
}
function clip(a, { y, h }, fn) {
  const { g } = a;
  g.save(); g.beginPath(); g.rect(PADL, y, a.plotW, h); g.clip();
  fn();
  g.restore();
}
function rightLabel(a, text, y, color = C.t3) {
  const { g } = a;
  g.font = font(10); g.fillStyle = color; g.textAlign = 'left'; g.textBaseline = 'middle';
  g.fillText(String(text), PADL + a.plotW + 5, y);
}
function inLaneText(a, text, x, y, { align = 'left', color = C.t3, box = false } = {}) {
  const { g } = a;
  g.font = font(9.5); g.textAlign = align; g.textBaseline = 'alphabetic';
  if (box) {
    const w = g.measureText(text).width;
    g.fillStyle = 'rgba(18,22,27,0.85)';
    g.fillRect(align === 'right' ? x - w - 2 : x - 2, y - 10, w + 4, 13);
  }
  g.fillStyle = color; g.fillText(text, x, y);
}

// Step series: an end-of-round value holds over [r, r + 1), a during-round value over [r − 1, r].
function stepPath(a, val, shift) {
  const { X, first, last } = a;
  const end = a.rep.metrics.length;
  const pts = [];
  for (let r = first; r <= last; r++) {
    const v = val(r);
    if (v === null) { pts.push(null); continue; }
    pts.push([X(r + shift), v], [X(Math.min(r + shift + 1, end)), v]);
  }
  return pts;
}
const EOR = 0, INR = -1;
function strokeSteps(g, pts) {
  g.beginPath();
  let pen = false;
  for (const p of pts) {
    if (p === null) { pen = false; continue; }
    pen ? g.lineTo(p[0], p[1]) : g.moveTo(p[0], p[1]);
    pen = true;
  }
  g.stroke();
}
function fillBetween(g, top, base) {
  g.beginPath();
  top.forEach((p, i) => (i ? g.lineTo(p[0], p[1]) : g.moveTo(p[0], p[1])));
  for (let i = base.length - 1; i >= 0; i--) g.lineTo(base[i][0], base[i][1]);
  g.closePath(); g.fill();
}

/* ---------- servers ---------- */

const BAND_COLOR = { collapsed: ['rgba(75,85,99,0.45)', 'collapsed'], recovering: ['rgba(224,175,104,0.35)', 'recovering'], recovered: ['rgba(61,214,196,0.22)', 'recovered'] };

function drawServers(a, lane) {
  const { g, rep, c, X, d0, d1, last } = a;
  const { y, h } = lane;
  const n = c.n;
  const ax = serverAxis(rep, n);
  const Y = v => y + h - ((Math.max(v, ax.lo) - ax.lo) / (n - ax.lo)) * h;
  const m = r => rep.metrics[r - 1];
  panel(a, lane);
  clip(a, lane, () => {
    const nb = stepPath(a, r => Y(m(r).nonbot_logs), EOR);
    g.fillStyle = 'rgba(61,214,196,0.22)';
    fillBetween(g, nb, nb.map(([x]) => [x, Y(ax.lo)]));
    const bl = stepPath(a, r => Y(n - m(r).blocked), INR);
    g.fillStyle = 'rgba(122,162,247,0.40)';
    fillBetween(g, bl.map(([x]) => [x, Y(n)]), bl);
    if (S.smr.live && S.smr.program && last < d1) {
      g.strokeStyle = 'rgba(122,162,247,0.6)'; g.lineWidth = 1.2; g.setLineDash([3, 3]);
      g.beginPath();
      for (let r = last + 1; r <= Math.floor(d1); r++) {
        const yy = Y(n - ((programBeta(S.smr.program, r) ?? 0) / 100) * n);
        r === last + 1 ? g.moveTo(X(r), yy) : g.lineTo(X(r), yy);
      }
      g.stroke(); g.setLineDash([]);
    }
    g.strokeStyle = C.guide; g.lineWidth = 1; g.setLineDash([5, 4]);
    for (const v of [ax.q3, ax.half]) {
      if (v <= ax.lo) continue;
      g.beginPath(); g.moveTo(PADL, Y(v) + 0.5); g.lineTo(PADL + a.plotW, Y(v) + 0.5); g.stroke();
    }
    g.setLineDash([]);
    g.strokeStyle = C.hi; g.lineWidth = 1.5;
    strokeSteps(g, stepPath(a, r => Y(m(r).useful), INR));
    for (const band of phaseBands(rep, c)) {
      if (band.to < d0 || band.from > d1) continue;
      const x0 = Math.max(PADL, X(band.from)), x1 = Math.min(PADL + a.plotW, X(band.to));
      if (x1 <= x0) continue;
      const [fill, label] = BAND_COLOR[band.kind];
      g.fillStyle = fill; g.fillRect(x0, y + h - 4, x1 - x0, 4);
      if (x1 - x0 > label.length * 6 + 8) inLaneText(a, label, x0 + 4, y + h - 7, { color: C.t1 });
    }
  });
  inLaneText(a, '⌈3n/4⌉', PADL + a.plotW - 6, Y(ax.q3) + 12, { align: 'right', color: C.guide, box: true });
  if (ax.half > ax.lo) inLaneText(a, 'n/2', PADL + a.plotW - 6, Y(ax.half) - 4, { align: 'right', color: C.guide, box: true });
  drawRStar(a, lane);
  rightLabel(a, n, Y(n) + 5);
  rightLabel(a, ax.q3, Y(ax.q3), C.guide);
  if (ax.half > ax.lo) rightLabel(a, ax.half, Y(ax.half), C.guide);
  rightLabel(a, ax.lo, y + h - 4);
  laneLabel(g, 'servers', y, 'SERVERS', 'who can take part?',
    'Teal: non-⊥ logs · white: useful · blue: blocked.\nGuides: Lemma 2.2 (n/2 and ⌈3n/4⌉).', ax.cropped ? 'axis n/2–n (cropped)' : 'axis 0–n');
}

// r* is the engine's recovered-round predicate (rStarScan); with a release the law card reads R off it.
function drawRStar(a, { y, h }) {
  const { g, rep, c, X, d0, d1 } = a;
  if (c.proto !== 'recovery' || !c.T) return;
  const rStar = rStarScan(rep, c.n, c.T);
  if (rStar === null || rStar < d0 || rStar > d1) return;
  const x = X(rStar);
  g.strokeStyle = C.teal; g.lineWidth = 1.2; g.setLineDash([2, 2]);
  g.beginPath(); g.moveTo(x + 0.5, y); g.lineTo(x + 0.5, y + h); g.stroke(); g.setLineDash([]);
  const text = `r* ${rStar}`;
  g.font = font(10, 600);
  const w = g.measureText(text).width;
  g.fillStyle = C.panel; g.fillRect(x + 3, y + 2, w + 6, 13);
  g.fillStyle = C.teal; g.textAlign = 'left'; g.textBaseline = 'alphabetic'; g.fillText(text, x + 6, y + 12);
  const release = releaseRound(rep, c.n);
  vp.marks.push({ x, y0: y, y1: y + h, tip: `r* ${rStar}: recovered round (useful servers agree; every later window end clean).` + (release !== null ? `\nR = r* − release = ${rStar - release}.` : '') });
}

/* ---------- resets · rollbacks ---------- */

function drawResets(a, lane) {
  const { g, rep, X, first, last } = a;
  const { y, h } = lane;
  const rows = recRows(rep);
  panel(a, lane);
  g.font = font(9.5); g.fillStyle = C.t3; g.textAlign = 'left'; g.textBaseline = 'alphabetic';
  g.fillText('resets · rollbacks', 8, y + 12);
  if (!rows) return;
  const colW = Math.max(1.5, X(first + 1) - X(first));
  clip(a, lane, () => {
    for (let r = first; r <= last; r++) {
      if (rows[r - 1].reset > 0) { g.strokeStyle = C.amber; g.lineWidth = 1; g.strokeRect(X(r) + 0.5, y + 2.5, Math.max(colW - 1, 1), h / 2 - 3); }
    }
    g.fillStyle = C.red;
    for (const rb of rollbackRounds(rows)) if (rb.round >= first && rb.round <= last) g.fillRect(X(rb.round) - 1, y + h / 2, Math.max(colW, 2), h / 2 - 2);
  });
  let resetRounds = 0, rollbacks = 0, depth = 0;
  for (const row of rows) { if (row.reset > 0) resetRounds++; rollbacks += row.rollbacks; depth = Math.max(depth, row.rollback_depth); }
  if (!resetRounds && !rollbacks) { inLaneText(a, 'none', PADL + 6, y + h - 4); return; }
  const plural = (k, w) => `${k} ${w}${k === 1 ? '' : 's'}`;
  const text = `armed resets in ${plural(resetRounds, 'round')} · ${plural(rollbacks, 'server rollback')}${rollbacks ? ` · depth ≤ ${depth}` : ''}`;
  inLaneText(a, text, PADL + a.plotW - 6, y + h - 4, { align: 'right', box: true });
  g.font = font(9.5);
  placeHint('resets', PADL + a.plotW - 26 - g.measureText(text).width, y + 1, 'Hollow amber: servers armed to reset · filled red: rollbacks at window ends.');
}

/* ---------- spread ---------- */

const HIGHLIGHT = { none: null, block: { color: C.tint, width: 1.4, alpha: 1 }, command: { color: '#ffffff', width: 2.2, alpha: 1 } };

function drawSpread(a, lane) {
  const { g, rep, c, X, d0, d1, blockCmds } = a;
  const { y, h } = lane;
  panel(a, lane);
  const top = y + 20, ph = h - 22;
  const Y = f => top + ph - f * ph;
  const visible = [];
  rep.commands.forEach((cmd, i) => {
    const pts = cmd.spread;
    if (!pts || !pts.length) return;
    if (pts[pts.length - 1].round < d0 || pts[0].round > d1) return;
    visible.push(i);
  });
  const levels = visible.map(i => cmdHighlight(i, S.smr.sel, blockCmds));
  const draw = (i, rank, level) => {
    const age = visible.length - 1 - rank;
    const hl = HIGHLIGHT[level];
    g.globalAlpha = hl ? hl.alpha : age < 24 ? 0.95 - age * 0.032 : 0.16;
    g.strokeStyle = hl ? hl.color : C.teal;
    g.lineWidth = hl ? hl.width : 1;
    g.beginPath();
    spreadCurve(rep.commands[i]).forEach((p, j) => {
      const yy = Y(p.useful_total ? p.useful_holders / p.useful_total : 0);
      j === 0 ? g.moveTo(X(p.round), yy) : g.lineTo(X(p.round), yy);
    });
    // a dark halo keeps the one white curve readable among its block's tinted ones
    if (level === 'command') { g.save(); g.strokeStyle = '#0f1216'; g.lineWidth = 5; g.stroke(); g.restore(); }
    g.stroke();
  };
  clip(a, lane, () => {
    // normal curves, then the block's tint, then the one selected command on top
    for (const pass of ['none', 'block', 'command']) visible.forEach((i, rank) => { if (levels[rank] === pass) draw(i, rank, pass); });
  });
  g.globalAlpha = 1;
  const st = spreadStats(rep.commands, c.n);
  const when = S.smr.end ? 'at run end' : `at r${rep.metrics.length}`;
  inLaneText(a, st.measured
    ? `${when}: T_B ${st.median}r median · ${st.p90}r p90 · ${st.multiple.toFixed(1)}× log₂n` + (st.spreading ? ` · ${st.spreading} still spreading` : '')
    : `${when}: ${st.spreading ? `${st.spreading} still spreading, none at every server yet` : 'no command delivered yet'}`, PADL + 6, y + 12);
  rightLabel(a, 1, Y(1) + 2);
  rightLabel(a, 0, Y(0) - 2);
  laneLabel(g, 'spread', y, 'SPREAD', 'how fast does it spread?', 'Share of useful servers holding each command, until one round after every log has it.');
  $('vpSpreadBtn').style.top = (y + 2) + 'px';
}

/* ---------- oldest unsettled age vs T ---------- */

function drawAge(a, lane) {
  const { g, c, X, first, last } = a;
  const { y, h } = lane;
  const ages = agesOf(c);
  panel(a, lane);
  let top = c.T * 1.25;
  for (let r = first; r <= last; r++) if (ages[r - 1]) top = Math.max(top, ages[r - 1].full);
  top = nice(top);
  const Y = v => y + h - (Math.min(v, top) / top) * h;
  const val = key => r => (ages[r - 1] ? Y(ages[r - 1][key]) : null);
  clip(a, lane, () => {
    g.fillStyle = 'rgba(247,118,142,0.05)'; g.fillRect(PADL, y, a.plotW, Y(c.T) - y);
    g.strokeStyle = C.red; g.lineWidth = 1; g.setLineDash([4, 3]);
    g.beginPath(); g.moveTo(PADL, Y(c.T) + 0.5); g.lineTo(PADL + a.plotW, Y(c.T) + 0.5); g.stroke(); g.setLineDash([]);
    g.strokeStyle = C.faint; g.lineWidth = 1; strokeSteps(g, stepPath(a, val('full'), EOR));
    g.strokeStyle = C.age; g.lineWidth = 1.5; strokeSteps(g, stepPath(a, val('contested'), EOR));
    for (let r = first; r <= last; r++) {
      const v = ages[r - 1];
      if (!v) continue;
      if (v.contested > c.T) { g.fillStyle = 'rgba(247,118,142,0.65)'; g.fillRect(X(r), Y(v.contested), X(Math.min(r + 1, a.rep.metrics.length)) - X(r), Y(c.T) - Y(v.contested)); }
      else if (v.full > c.T) { g.fillStyle = 'rgba(247,118,142,0.35)'; g.fillRect(X(r), Y(c.T) - 6, Math.max(X(Math.min(r + 1, a.rep.metrics.length)) - X(r), 1.5), 6); }
    }
  });
  inLaneText(a, 'contested (main) · full (faint) · red: over T', PADL + a.plotW - 6, y + h - 4, { align: 'right', box: true });
  rightLabel(a, `T ${c.T}`, Y(c.T), C.red);
  rightLabel(a, 0, y + h - 4);
  laneLabel(g, 'age', y, 'AGE vs T', 'how close to T?', 'Oldest log entry whose position is still open, against T.');
}

/* ---------- backlog by stage ---------- */

const STAGE_STYLE = {
  client: ['#4a535f', 0.8], ordering: ['#5d7bb5', 0.6], spreading: ['#5d7bb5', 0.6],
  waiting: ['#9aa3ae', 0.5], frozen: [C.amber, 0.55],
};
function stageLabel(key, proto, live) {
  if (key === 'client') return 'client';
  if (key === 'ordering') return 'being ordered';
  if (key === 'spreading') return 'spreading';
  if (key === 'frozen') return 'frozen in P, waiting to execute';
  if (proto === 'recovery') return live ? 'ordered, waiting for the freeze' : 'waiting (ordering + freeze)';
  return live ? 'ordered, waiting to commit' : 'waiting to commit';
}
const SHORT = { client: 'client', ordering: 'ordering', spreading: 'spreading', waiting: 'waiting', frozen: 'frozen' };

function drawBacklog(a, lane) {
  const { g, rep, c, first, last, D } = a;
  const { y, h } = lane;
  panel(a, lane);
  const s = stageSeries(rep, c.proto, c.T, fixedLive(c));
  vp.stages = s;
  let maxP = 1;
  for (let r = first; r <= last; r++) maxP = Math.max(maxP, D.pending[r]);
  const gs = maxP >= 200 ? 100 : maxP >= 20 ? 10 * Math.ceil(maxP / 100) : maxP >= 5 ? 2 : 1;
  const yMax = Math.max(gs, Math.ceil((maxP * 1.04) / (gs / 2)) * (gs / 2));
  const Y = v => y + h - (v / yMax) * h;
  g.strokeStyle = C.grid; g.lineWidth = 1;
  for (let v = gs; v < yMax; v += gs) {
    g.beginPath(); g.moveTo(PADL, Y(v) + 0.5); g.lineTo(PADL + a.plotW, Y(v) + 0.5); g.stroke();
    rightLabel(a, v, Y(v));
  }
  g.font = font(9.5); g.fillStyle = C.t3; g.textAlign = 'left'; g.textBaseline = 'alphabetic';
  g.fillText('cmds', PADL + a.plotW + 5, y + 9);
  clip(a, lane, () => {
    const cum = new Int32Array(rep.metrics.length + 2);
    let base = stepPath(a, () => Y(0), EOR);
    for (const key of [...s.keys].reverse()) {
      const [col, alpha] = STAGE_STYLE[key];
      for (let r = first; r <= last; r++) cum[r] += s.counts[key][r];
      const top = stepPath(a, r => Y(cum[r]), EOR);
      g.globalAlpha = alpha; g.fillStyle = col;
      fillBetween(g, top, base);
      base = top;
    }
    g.globalAlpha = 1;
    g.strokeStyle = C.purple; g.lineWidth = 1.3;
    strokeSteps(g, stepPath(a, r => Y(D.pending[r]), EOR));
  });
  const merged = c.proto !== 'extended' && !fixedLive(c);
  laneLabel(g, 'backlog', y, 'BACKLOG', 'where is it waiting?',
    'Uncommitted commands by stage; the top line is the backlog.' + (merged ? '\nNo position-fixed record in this run: ordering and waiting are one stage.' : '') +
    (s.lastP && s.lastP.size === null ? `\nNewest P (r${s.lastP.boundary}): membership unknown, ${s.lastP.forkK} checkpoints; its commands show as waiting.` : '') +
    (s.clamped ? `\n${s.clamped} commands reached a later stage before their position was fixed.` : ''),
    merged ? 'ordering + waiting merged' : 'by stage · unit: commands');
}

function drawLegend(a, { y }) {
  const { g, c } = a;
  const s = vp.stages;
  let x = PADL + 4;
  g.font = font(9.5); g.textBaseline = 'alphabetic'; g.textAlign = 'left';
  for (const key of s.keys) {
    const [col, alpha] = STAGE_STYLE[key];
    g.globalAlpha = alpha; g.fillStyle = col; g.fillRect(x, y + 1, 14, 9); g.globalAlpha = 1;
    const label = stageLabel(key, c.proto, fixedLive(c));
    g.fillStyle = C.t3; g.fillText(label, x + 18, y + 10);
    x += 30 + g.measureText(label).width;
  }
  g.strokeStyle = C.purple; g.lineWidth = 2;
  g.beginPath(); g.moveTo(x, y + 6); g.lineTo(x + 16, y + 6); g.stroke();
  g.fillStyle = C.t3; g.fillText('total = backlog', x + 20, y + 10);
}

/* ---------- block tiles ---------- */

const TILE = {
  committed: { fill: '#142321', edge: '#1d5a53', ink: C.teal },
  drain: { fill: '#241f14', edge: '#5c4a1e', ink: C.amber },
  rollback: { fill: '#241f14', edge: '#8a6a2c', ink: '#c49a52' },
  empty: { fill: '#161a20', edge: '#232932', ink: '#6f7986' },
};

// Committed-sequence positions: recovery reads the chain delta every step, compact has them
// only while its certificate feed is loaded (the block drawer).
const entriesFor = c => (c.proto === 'recovery' ? S.smr.chain?.entries : forestView()?.entries);
function tileMargins(rep, c, blocks) {
  const entries = entriesFor(c);
  if (!entries || !entries.length) return null;
  const ll = lifelines(rep.commands, entries, c.T, c.proto);
  if (!ll) return null;
  const out = new Map();
  for (const b of blocks) {
    if (!b.size || b.from + b.size > ll.rows.length) continue;
    const m = blockMinMargin(ll.rows.slice(b.from, b.from + b.size), c.proto);
    if (m) out.set(b.m, m);
  }
  return out;
}
const signed = v => (v < 0 ? '−' : '+') + Math.abs(v) + 'r';
// the longest candidate that fits a tile of width w, else the last one
const fitting = (g, w, texts) => texts.find(t => g.measureText(t).width <= w - 8) ?? texts[texts.length - 1];

function drawTiles(a, lane) {
  const { g, rep, c, X, blocks, sel } = a;
  const { y, h } = lane;
  const margins = tileMargins(rep, c, blocks);
  vp.tileMargins = margins;
  const nextLo = blocks.length * c.T;
  const next = { m: blocks.length + 1, round: nextLo + c.T, state: 'next', label: c.proto === 'recovery' ? `b${nextLo + c.T}` : `r${nextLo + 1}–${nextLo + c.T}` };
  g.textAlign = 'center'; g.textBaseline = 'alphabetic';
  for (const b of [...blocks, next]) {
    const x0 = Math.max(X(b.round - c.T), PADL), x1 = Math.min(X(b.round), PADL + a.plotW);
    if (x1 - x0 < 4) continue;
    const bx = x0 + 1, bw = x1 - x0 - 2, cx = bx + bw / 2;
    if (b.state === 'next') {
      g.strokeStyle = '#333b46'; g.setLineDash([3, 2]); g.lineWidth = 1;
      g.strokeRect(bx + 0.5, y + 0.5, bw - 1, h - 1); g.setLineDash([]);
      if (bw > 20) { g.font = font(9.5); g.fillStyle = '#6f7986'; g.fillText(fitting(g, bw, [b.label, '']), cx, y + 12); g.fillStyle = C.mute; g.fillText('next', cx, y + 25); }
      continue;
    }
    const isSel = sel && sel.m === b.m;
    const st = TILE[b.state];
    const count = c.proto === 'compact' ? b.count : b.size;
    g.fillStyle = isSel ? '#1d2b2a' : count ? st.fill : TILE.empty.fill; g.fillRect(bx, y, bw, h);
    g.strokeStyle = isSel ? C.hi : st.edge; g.lineWidth = isSel ? 1.5 : 1;
    g.strokeRect(bx + 0.5, y + 0.5, bw - 1, h - 1);
    vp.hits.push({ x: bx, y, w: bw, h, block: b });
    if (bw < 20) continue;
    g.font = font(9.5, count ? 600 : 400);
    g.fillStyle = isSel ? C.hi : st.ink;
    g.fillText(fitting(g, bw, [`${b.label} · ${count}`, b.label, '']), cx, y + 12);
    g.font = font(9.5);
    const mm = margins?.get(b.m);
    const lat = b.latency ? `⌀${Math.round(b.latency.avg)}r` : null;
    const min = mm ? `min ${signed(mm.value)}` : null;
    const drain = b.state === 'drain' ? 'drain' : null;
    const sub = !count ? (b.state === 'rollback' ? 'rollback' : 'empty')
      : fitting(g, bw, [[lat, min, drain], [lat, min], [min ?? lat], ['']].map(xs => xs.filter(Boolean).join(' · ')));
    g.fillStyle = isSel ? C.hi : count ? (mm && mm.value < 0 ? C.red : C.t2) : C.mute;
    g.fillText(sub, cx, y + 25);
  }
}

// The selected block's life on the round axis, so band, bracket, curves and tile read as one
// object: first delivery → spread (median delivery → median every log) → fixed → frozen → executed.
function drawBrackets(a, placed) {
  const { rep, c, blocks, sel, life } = a;
  if (life) drawLife(a, placed, life, 1);
  const hov = vp.hoverBlock != null ? blocks.find(b => b.m === vp.hoverBlock) : null;
  if (hov && (!sel || hov.m !== sel.m)) {
    const hl = blockLife(rep, hov, c.proto, c.T);
    if (hl) drawLife(a, placed, hl, 0.4, false);
  }
}
function drawLife(a, placed, life, alpha, band = true) {
  const { g, X } = a;
  const { y } = placed.bracket;
  const x = r => Math.max(PADL, Math.min(PADL + a.plotW, X(r)));
  if (band) {
    const y0 = placed.servers.y, y1 = placed.tiles.y + placed.tiles.h;
    g.fillStyle = 'rgba(230,237,243,0.045)'; g.fillRect(x(life.from), y0, x(life.end) - x(life.from), y1 - y0);
  }
  const mid = y + 5;
  g.globalAlpha = alpha; g.strokeStyle = C.tint; g.fillStyle = C.tint; g.lineWidth = 1;
  g.beginPath(); g.moveTo(x(life.from), mid + 0.5); g.lineTo(x(life.end), mid + 0.5); g.stroke();
  const tick = (r, up, down) => { g.beginPath(); g.moveTo(x(r) + 0.5, mid - up); g.lineTo(x(r) + 0.5, mid + down); g.stroke(); };
  tick(life.from, 3, 3);
  if (life.spread[0] !== null && life.spread[1] !== null) g.fillRect(x(life.spread[0]), mid - 1.5, Math.max(2, x(life.spread[1]) - x(life.spread[0])), 4);
  if (life.fixed !== null) tick(life.fixed, 4, 4);
  if (life.frozen !== null) tick(life.frozen, 4, 6);
  tick(life.end, 4, 6);
  g.globalAlpha = 1;
}

/* ---------- markers, axis, cursor ---------- */

const EVENT_COLOR = { collapse: '#9ca3af', release: C.amber };

function drawEventMarkers(a, top, bottom) {
  const { g, X, d0, d1 } = a;
  for (const ev of timelineEvents(a.rep, a.c)) {
    if (!EVENT_COLOR[ev.kind] || ev.round < d0 || ev.round > d1) continue;
    const x = X(ev.round);
    g.strokeStyle = EVENT_COLOR[ev.kind]; g.lineWidth = 1; g.setLineDash([3, 3]);
    g.beginPath(); g.moveTo(x + 0.5, top); g.lineTo(x + 0.5, bottom); g.stroke(); g.setLineDash([]);
    inLaneText(a, ev.label, x + 4, top + 26, { color: EVENT_COLOR[ev.kind] });
    vp.marks.push({ x, y0: top, y1: bottom, tip: ev.tip });
  }
}

function cursorRound(rep) {
  if (vp.hover !== null) return vp.hover;
  if (vp.pinned !== null) return vp.pinned;
  return Math.min(rep.metrics.length, Math.ceil(vp.args ? vp.args.d1 : rep.metrics.length));
}

// Axis labels step by whole windows; the cursor is a dashed line broken around the tiles and the
// legend so it never prints over their text, labelled in a pill on the axis.
function drawAxisAndCursor(a, placed) {
  const { g, X, d0, d1, c, rep } = a;
  const ay = placed.axis.y;
  const unit = c.T || 10;
  const step = Math.max(unit, Math.ceil((d1 - d0) / 11 / unit) * unit);
  const cur = cursorRound(rep);
  const showCur = cur >= d0 && cur <= d1;
  g.font = font(10); g.textAlign = 'center'; g.textBaseline = 'alphabetic';
  for (let r = Math.ceil(d0 / step) * step; r <= d1; r += step) {
    g.strokeStyle = '#2c343e'; g.lineWidth = 1;
    g.beginPath(); g.moveTo(X(r) + 0.5, ay); g.lineTo(X(r) + 0.5, ay + 4); g.stroke();
    if (!showCur || Math.abs(X(r) - X(cur)) > 34) { g.fillStyle = C.t3; g.fillText('r' + r, X(r), ay + 15); }
  }
  if (!showCur) return;
  const x = X(cur) + 0.5;
  const gaps = [placed.tiles, placed.legend].filter(Boolean).map(l => [l.y - 1, l.y + l.h + 1]);
  g.strokeStyle = C.hi; g.globalAlpha = vp.pinned === cur || vp.hover === null ? 1 : 0.7; g.lineWidth = 1; g.setLineDash([2, 3]);
  let yy = placed.servers.y;
  for (const [g0, g1] of gaps) { g.beginPath(); g.moveTo(x, yy); g.lineTo(x, g0); g.stroke(); yy = g1; }
  g.beginPath(); g.moveTo(x, yy); g.lineTo(x, ay); g.stroke(); g.setLineDash([]);
  g.globalAlpha = 1;
  g.fillStyle = C.hi;
  g.beginPath(); g.roundRect(x - 19, ay + 3, 38, 15, 3); g.fill();
  g.fillStyle = '#0f1216'; g.font = font(10, 600); g.fillText('r' + cur, x, ay + 14);
}

/* ---------- readout line ---------- */

const word = (label, value, { cls = '', tip = '' } = {}) =>
  `<span class="ro"${tip ? ` data-tip="${escapeHtml(tip)}"` : ''}><span class="roK">${label}</span> <b class="${cls}">${value}</b></span>`;

function stageBar(s, r) {
  const total = s.keys.reduce((acc, k) => acc + s.counts[k][r], 0);
  const segs = total ? s.keys.map(k => {
    const [col, alpha] = STAGE_STYLE[k];
    return `<i style="width:${(s.counts[k][r] / total) * 100}%;background:${col};opacity:${Math.min(1, alpha + 0.25)}"></i>`;
  }).join('') : '';
  return `<span class="stageBar">${segs}</span>`;
}

function renderReadout(rep, c) {
  const r = cursorRound(rep);
  const m = rep.metrics[r - 1];
  if (!m) { $('vpReadout').innerHTML = ''; return; }
  const D = derive(rep, c.proto);
  const rows = recRows(rep);
  const n = c.n;
  const mem = S.smr.mem && S.smr.mem.length ? S.smr.mem[S.smr.mem.length - 1] : null;
  const level = memoryLevel(mem?.linear_bytes);
  const parts = [word('round', r, mem ? { tip: `simulator memory ${formatBytes(mem.linear_bytes)} (wasm, at r${mem.round})` } : {})];
  parts.push(word('blocked', Math.round((m.blocked / n) * 100) + '%', { cls: 'cBlue', tip: `during r${r}: ${m.blocked} of ${n} blocked` }));
  parts.push(word('useful', `${m.useful}/${n}`, { tip: `start of r${r}\nend: non-⊥ logs ${m.nonbot_logs} · distinct logs ${m.distinct_logs}` }));
  const age = agesOf(c)?.[r - 1];
  if (age) {
    parts.push(word('oldest unsettled', `${age.contested}r / T ${c.T}`, {
      cls: age.contested > c.T ? 'cRed' : '', tip: `end of r${r}: contested ${age.contested}r · full ${age.full}r`,
    }));
  }
  const s = stageSeries(rep, c.proto, c.T, fixedLive(c));
  const stageTip = `end of r${r}: ` + s.keys.map(k => `${SHORT[k]} ${s.counts[k][r]}`).join(' · ') +
    `\noldest waiting ${D.oldest[r]}r` + (c.proto === 'recovery' && c.T ? ` (2T = ${2 * c.T}r)` : '');
  parts.push(`<span class="ro" data-tip="${escapeHtml(stageTip)}"><span class="roK">backlog</span> <b class="cPurple">${D.pending[r]}</b>${stageBar(s, r)}</span>`);
  const nc = neverCommitted(rep, c.proto);
  const commitTip = `end of r${r}: agreed prefix ${m.lcp_len}/${Math.max(m.max_log_len, m.max_executed_len)}` +
    (c.proto !== 'extended' ? ` · executed ${m.min_executed_len}–${m.max_executed_len}` : '') +
    (!S.smr.live && nc.total ? `\nnever committed ${nc.total} (in flight ${nc.inFlight}${c.proto === 'extended' ? `, stranded ${nc.stranded}` : ''}, dead ${nc.dead})` : '');
  parts.push(word(c.proto === 'extended' ? 'in every log' : 'committed', D.committed[r], { cls: 'cTeal', tip: commitTip }));
  // quiet by default: these appear only when something is off
  const row = rows ? rows[r - 1] : null;
  if (row && row.reset > 0) parts.push(word('resets', row.reset, { cls: 'cAmber', tip: `end of r${r}: servers in reset` }));
  const rb = rows && rows[r] ? rows[r] : null;
  if (rb && rb.rollbacks > 0) parts.push(word('rollbacks', rb.rollbacks, { cls: 'cRed', tip: `window end r${r}, up to ${rb.rollback_depth} windows back` }));
  if (level !== 'ok') parts.push(word('memory', formatBytes(mem.linear_bytes), { cls: level === 'high' ? 'cRed' : 'cAmber', tip: 'simulator memory nears the page limit (wasm32: 4 GiB at most)' }));
  // compact servers execute at different rounds by design; recovery only at window ends
  if (c.proto === 'recovery' && m.min_executed_len !== m.max_executed_len) {
    parts.push(word('stragglers', `executed ${m.min_executed_len}–${m.max_executed_len}`, { cls: 'cAmber', tip: `end of r${r}: executed lengths differ` }));
  }
  $('vpReadout').innerHTML = parts.join('');
}

/* ---------- interaction ---------- */

function roundAtEvent(ev) {
  const a = vp.args;
  if (!a) return null;
  const rect = $('vpCanvas').getBoundingClientRect();
  const frac = (ev.clientX - rect.left - PADL) / a.plotW;
  const r = Math.round(a.d0 + frac * (a.d1 - a.d0));
  return Math.max(1, Math.min(a.rep.metrics.length, r));
}
const redraw = () => { if (vp.rep && ctxOf()) renderViewport(vp.rep); };

function pan(deltaRounds) {
  const rep = vp.rep, c = ctxOf();
  if (!rep || !c) return;
  const total = rep.metrics.length + lookahead(spanOf(c)), span = spanOf(c);
  if (total <= span) return;
  const [d0] = domain(rep.metrics.length, span);
  const lo = Math.min(Math.max(d0 + deltaRounds, 0), total - span);
  vp.lo = lo;
  vp.follow = lo >= total - span - 0.5;
  redraw();
}

const hitAt = (x, y) => vp.hits.find(h => x >= h.x && x <= h.x + h.w && y >= h.y && y <= h.y + h.h);

$('vpCanvas').addEventListener('mousemove', ev => {
  const a = vp.args;
  if (!a) return;
  if (vp.drag) {
    pan(-((ev.clientX - vp.drag.x) / a.plotW) * (a.d1 - a.d0));
    vp.drag.x = ev.clientX;
    vp.drag.moved = true;
    return;
  }
  const rect = $('vpCanvas').getBoundingClientRect();
  const x = ev.clientX - rect.left, y = ev.clientY - rect.top;
  const block = hitAt(x, y);
  vp.hover = x >= PADL ? roundAtEvent(ev) : null;
  vp.hoverBlock = block ? block.block.m : null;
  redraw();
  const mark = vp.marks && vp.marks.find(m => Math.abs(m.x - x) < 4 && y >= m.y0 && y <= m.y1 && m.tip);
  if (block) showTip(blockTip(block.block, a.c), ev.clientX, ev.clientY);
  else if (mark) showTip(escapeHtml(mark.tip), ev.clientX, ev.clientY);
  else hideTip();
});
$('vpCanvas').addEventListener('mouseleave', () => { vp.hover = null; vp.hoverBlock = null; vp.drag = null; hideTip(); redraw(); });
$('vpCanvas').addEventListener('mousedown', ev => { vp.drag = { x: ev.clientX, moved: false }; });
window.addEventListener('mouseup', () => { vp.drag = null; });
$('vpCanvas').addEventListener('wheel', ev => {
  const d = Math.abs(ev.deltaX) > Math.abs(ev.deltaY) ? ev.deltaX : ev.shiftKey ? ev.deltaY : 0;
  if (!d || !vp.args) return;
  ev.preventDefault();
  pan((d / vp.args.plotW) * (vp.args.d1 - vp.args.d0));
}, { passive: false });
$('vpCanvas').addEventListener('click', ev => {
  if (vp.drag && vp.drag.moved) return;
  const a = vp.args;
  if (!a) return;
  const rect = $('vpCanvas').getBoundingClientRect();
  const hit = hitAt(ev.clientX - rect.left, ev.clientY - rect.top);
  if (hit) {
    const next = nextSelection({ blockSel: S.smr.blockSel ?? null, sel: S.smr.sel }, { block: hit.block.m });
    S.smr.blockSel = next.blockSel;
    handlers.openBlock(next.blockSel === null ? null : hit.block);
    if (next.sel !== S.smr.sel) handlers.selectCmd(next.sel);
    redraw();
    return;
  }
  if (ev.clientX - rect.left < PADL) return;
  const r = roundAtEvent(ev);
  vp.pinned = vp.pinned === r ? null : r;
  redraw();
});

function overviewRound(ev) {
  const o = vp.overview;
  if (!o) return null;
  const rect = $('vpOverview').getBoundingClientRect();
  return ((ev.clientX - rect.left - PADL) / (o.W - PADL - PADR)) * o.total;
}
function seekOverview(ev) {
  const r = overviewRound(ev), c = ctxOf();
  if (r === null || !c || !vp.rep) return;
  const span = spanOf(c), total = vp.rep.metrics.length + lookahead(span);
  if (total <= span) return;
  const lo = Math.min(Math.max(r - span / 2, 0), total - span);
  vp.lo = lo;
  vp.follow = lo >= total - span - 0.5;
  redraw();
}
let overviewDrag = false;
$('vpOverview').addEventListener('mousedown', ev => { overviewDrag = true; seekOverview(ev); });
window.addEventListener('mousemove', ev => { if (overviewDrag) seekOverview(ev); });
window.addEventListener('mouseup', () => { overviewDrag = false; });
$('vpLive').addEventListener('click', () => { vp.follow = true; redraw(); });
$('vpSpreadBtn').addEventListener('click', () => handlers.openSpread());

// Two lines: what the tile holds, then where its commands were on the round axis.
function blockTip(b, c) {
  const count = c.proto === 'compact' ? b.count : b.size;
  const mm = vp.tileMargins?.get(b.m);
  let head = `<b>${escapeHtml(b.label)}</b> · ${count} ${c.proto === 'compact' ? 'committed' : 'entries'}`;
  if (b.latency) head += ` · ⌀${Math.round(b.latency.avg)}r`;
  if (mm) head += ` · min margin ${signed(mm.value)} at #${mm.pos}`;
  if (b.state === 'rollback') head += ' · rolled back';
  const life = blockLife(vp.rep, b, c.proto, c.T);
  if (!life) return head;
  const parts = [`spread r${life.spread[0]}–r${life.spread[1] ?? '?'}`];
  if (life.fixed !== null) parts.push(`fixed r${life.fixed}`);
  if (life.frozen !== null) parts.push(`frozen r${life.frozen}`);
  parts.push(c.proto === 'recovery' ? `executed r${life.end}` : `committed by r${life.end}`);
  return head + '<br>' + parts.join(' · ');
}

window.addEventListener('resize', redraw);

/* ---------- run summary ---------- */

const sum = { scope: 'run', key: null };
for (const b of document.querySelectorAll('#sumScope .ct')) {
  b.addEventListener('click', () => {
    sum.scope = b.dataset.scope;
    for (const x of document.querySelectorAll('#sumScope .ct')) x.classList.toggle('on', x === b);
    redraw();
  });
}
const card = (title, tip, body, cls = '') => `<div class="sumCard ${cls}"><div class="llCap">${title}<span class="qHint" data-tip="${escapeHtml(tip)}">?</span></div>${body}</div>`;
const statLine = t => `<div class="sumStat">${t}</div>`;
const pct = x => (x === null ? '–' : Math.round(100 * x) + '%');

function renderSummary(rep, c) {
  const [d0, d1] = domain(rep.metrics.length, spanOf(c));
  const entries = entriesFor(c);
  const lo = Math.max(1, Math.ceil(d0)), hi = Math.min(rep.metrics.length, Math.floor(d1));
  const key = [rep.metrics.length, rep.commands.length, sum.scope, sum.scope === 'view' ? `${lo}-${hi}` : '', entries ? entries.length : -1, c.kind, S.smr.mem?.length ?? -1, S.smr.seqRound ?? -1].join(':');
  if (sum.key === key && sum.rep === rep) return;
  sum.key = key; sum.rep = rep;
  const scope = sum.scope === 'view' ? inViewSet(rep, lo, hi) : null;
  const inScope = i => !scope || scope.has(i);
  const n = scope ? scope.size : rep.commands.length;
  $('sumRange').textContent = sum.scope === 'view' ? `delivered r${lo}–r${hi} · ${n} cmds` : `r1–r${rep.metrics.length} · ${n} cmds`;
  const W = 250;
  const cards = [];

  // spread to every server (T_B)
  const scoped = rep.commands.filter((_, i) => inScope(i));
  const st = spreadStats(scoped, c.n);
  const tb = scoped.filter(x => x.delivered_round != null && x.all_logs_round != null).map(x => x.all_logs_round - x.delivered_round);
  // a surge's few slow spreads would flatten the rest: the range stops at 2·p90, outliers fold into the last bin
  const tbHi = Math.max(4, Math.min(tb.reduce((m, v) => Math.max(m, v), 0), 2 * (st.p90 ?? 0) + 2));
  cards.push(card('SPREAD', 'Rounds from delivery to every non-⊥ log (T_B), per command.',
    (tb.length ? histSvg(tb, { W, H: 70, lo: 0, hi: tbHi, color: C.teal, ticks: ticksFor(0, tbHi), tip: (l, k) => `T_B ${l}r: ${k}` }) : '<div class="bdDim">no command in every log yet</div>') +
    statLine(st.measured ? `median ${st.median}r · p90 ${st.p90}r · ${st.spreading} still spreading` : `${st.spreading} still spreading`)));

  // latency by stage
  const live = fixedLive(c);
  const lat = stageLatencies(rep, c.proto, c.T, live, scope);
  const mx = Math.max(1, ...lat.map(x => x.p90 ?? 0));
  const LX = 66, LW = W - LX - 62;
  let svg = '';
  lat.forEach((x, k) => {
    const y = 4 + k * 18, [col, alpha] = STAGE_STYLE[x.key];
    svg += `<text x="0" y="${y + 9}" fill="#8b95a1" font-size="10" font-family="${MONO}">${SHORT[x.key]}</text>`;
    if (x.byDefinition && c.proto === 'recovery') {
      svg += `<text x="${LX}" y="${y + 9}" fill="#8b95a1" font-size="10" font-family="${MONO}">= T (${c.T}) by definition</text>`;
    } else if (x.median !== null) {
      svg += `<rect x="${LX}" y="${y + 1}" width="${Math.max(1, (x.median / mx) * LW)}" height="10" fill="${col}" fill-opacity="${Math.min(1, alpha + 0.25)}"/>` +
        `<line x1="${LX + (x.p90 / mx) * LW}" y1="${y}" x2="${LX + (x.p90 / mx) * LW}" y2="${y + 12}" stroke="${C.hi}" stroke-width="1.5"/>` +
        `<text x="${LX + LW + 6}" y="${y + 9}" fill="#aeb6c0" font-size="10" font-family="${MONO}">${x.median} · ${x.p90}</text>`;
    }
  });
  const committed = lat[0]?.n ?? 0;
  cards.push(card('LATENCY BY STAGE', `Median (bar) and p90 (tick) per stage over ${committed} committed commands.` + (c.proto !== 'extended' && !live ? '\nNo position-fixed record in this run: ordering and waiting are one stage.' : ''),
    `<svg width="${W}" height="${4 + lat.length * 18}">${svg}</svg>` + statLine('median · p90, rounds')));

  // commit margin and ordering need the committed sequence; the margin also needs position fixed
  if (c.proto !== 'extended') {
    const ll = entries && entries.length ? lifelines(rep.commands, entries, c.T, c.proto) : null;
    const marginTip = 'Deadline − position fixed per committed entry, as on the tiles.';
    const stale = staleNote(c.proto, S.smr.seqRound ?? null, rep.metrics.length);
    const staleLine = stale ? `<div class="sumStat stale">${stale}</div>` : '';
    if (!live) cards.push(card('COMMIT MARGIN', marginTip + '\nNeeds position fixed, recorded by live sessions only.', '<div class="bdDim">not recorded in this run</div>'));
    else if (!ll) cards.push(card('COMMIT MARGIN', marginTip, `<div class="bdDim">${c.proto === 'compact' ? 'open a block once to load the committed sequence' : 'no commits yet'}</div>`));
    else {
      const ms = ll.rows.filter(r => r.cmd >= 0 && inScope(r.cmd)).map(r => shownMargin(r, c.proto).value).filter(v => v !== null);
      const mLo = Math.min(0, ...ms), mHi = Math.max(2 * c.T, ...ms);
      const bin = Math.max(1, Math.ceil((mHi - mLo + 1) / 28));
      const below = ms.filter(v => v < c.T / 4).length, neg = ms.filter(v => v < 0).length;
      cards.push(card('COMMIT MARGIN', marginTip,
        (ms.length ? histSvg(ms, { W, H: 70, lo: mLo, hi: mHi, bin, color: v => (v < 0 ? C.red : v < c.T / 4 ? '#9aa3ae' : C.teal),
          marks: [[0, C.red, ''], [c.T / 4, C.guide, '3 2']], ticks: [0, c.T], xlab: v => String(v), tip: (l, k) => `${l}r: ${k}` }) : '<div class="bdDim">nothing committed in scope</div>') +
        statLine(ms.length ? `min ${signed(Math.min(...ms))} · ${below} below T/4 · ${neg} below 0` : '') + staleLine));
    }
    if (!entries || !entries.length) cards.push(card('ORDERING', 'Commit rank − submission rank; same-round submissions tied.', `<div class="bdDim">${live ? 'no committed sequence loaded yet' : 'needs the live committed sequence'}</div>`));
    else {
      const fullOrder = committedOrder(rep, entries);
      const order = fullOrder.filter(inScope);
      const d = displacements(rep, order);
      cards.push(card('ORDERING', 'Commit rank − submission rank over committed commands; same-round submissions tied.',
        (d.length ? histSvg(d, { W, H: 70, lo: -16, hi: 12, color: dispColor, ticks: [-15, -10, -5, 0, 5, 10], xlab: signedInt, tip: (l, k) => `${l}: ${k}` }) : '<div class="bdDim">nothing committed in scope</div>') +
        statLine(d.length ? `${pct(withinShare(d, 3))} within ±3 places · ${pct(withinShare(d, 0))} in place` : '') + staleLine));
      const f = fifoCheck(rep, fullOrder, scope);
      const chip = f.status === 'na' ? '<span class="fifoChip na"><i></i>not applicable</span>'
        : f.status === 'ok' ? '<span class="fifoChip ok"><i></i>holds</span>' : `<span class="fifoChip fail"><i></i>${f.violations} out of order</span>`;
      cards.push(card('PER-CLIENT FIFO', 'Per client, commands commit in issue (sn) order.', chip +
        statLine(f.multi ? `${f.multi} of ${f.clients} clients issued 2+` : `${f.clients} clients, 1 command each`) + statLine(`${f.pairs} pairs checked`) + staleLine));
    }
  }
  if (S.smr.mem && S.smr.mem.length) cards.push(memoryCard(S.smr.mem, W));
  $('sumCards').innerHTML = cards.join('');
}
function ticksFor(lo, hi) {
  const step = Math.max(1, Math.ceil((hi - lo) / 6));
  const out = [];
  for (let v = lo; v <= hi; v += step) out.push(v);
  return out;
}

const MEM_CATS = [['log', 'log', C.teal], ['executed', 'executed', '#9aa3ae'], ['sn', 'sn map', '#5d7bb5'], ['checkpoint', 'checkpoint', C.amber], ['certs', 'certificates', C.purple]];

// Per-node bytes over time (median solid, max faint), the median node's categories, the simulator.
function memoryCard(mem, W) {
  const last = mem[mem.length - 1];
  const pts = mem.filter(m => m.median !== null);
  const H = 36, r0 = pts[0]?.round ?? 0, r1 = Math.max(last.round, r0 + 1);
  const top = Math.max(1, ...pts.map(m => m.max));
  const xy = key => pts.map(m => `${((m.round - r0) / (r1 - r0)) * W},${H - 2 - (m[key] / top) * (H - 4)}`).join(' ');
  const spark = `<svg width="${W}" height="${H}"><polyline points="${xy('max')}" fill="none" stroke="${C.teal}" stroke-opacity="0.35"/>` +
    `<polyline points="${xy('median')}" fill="none" stroke="${C.teal}" stroke-width="1.4"/></svg>`;
  const node = last.median_node;
  let rows = '';
  if (node) {
    const total = Math.max(1, last.median);
    MEM_CATS.forEach(([k, label, col], i) => {
      const y = 2 + i * 15;
      rows += `<text x="0" y="${y + 9}" fill="#8b95a1" font-size="10" font-family="${MONO}">${label}</text>` +
        `<rect x="84" y="${y + 1}" width="${Math.max(node[k] ? 1 : 0, ((W - 150) * node[k]) / total)}" height="9" fill="${col}" fill-opacity="0.8"/>` +
        `<text x="${W}" y="${y + 9}" text-anchor="end" fill="#aeb6c0" font-size="10" font-family="${MONO}">${formatBytes(node[k])}</text>`;
    });
  }
  const level = memoryLevel(last.linear_bytes);
  return card('MEMORY', 'Bytes: structural estimate (element size × count, maps at 1.5× entries), not allocator RSS.\nPer node: one server owning its own copy of everything it references.',
    (last.median !== null
      ? `<div class="ordBig">${formatBytes(last.median)}<span> per node · max ${formatBytes(last.max)}</span></div>` + spark +
        `<svg width="${W}" height="${2 + MEM_CATS.length * 15}">${rows}</svg>`
      : '<div class="bdDim">every server ⊥</div>') +
    statLine(`simulator <b class="${level === 'ok' ? '' : level === 'high' ? 'cRed' : 'cAmber'}">${formatBytes(last.linear_bytes)}</b> wasm · nodes ${formatBytes(last.simulator_node_bytes)}`) +
    (last.sharing !== null ? `<div class="sumStat" data-tip="all n servers apart (⊥ ones too) ÷ the simulator's bytes for the same n servers">sharing ×${Math.round(last.sharing)}</div>` : ''));
}
