// Single-value session timeline: the same viewport as the SMR page (fixed span,
// overview strip, follow-live, cursor, readout) with two lanes — value
// composition over rounds and the per-node timeline. Both read the stored
// trace; nothing here is estimated.
import { S, $, colorOf, BOT, BLOCKED, statesAt, roundInfo } from './state.js';
import { showTip, hideTip } from './tooltip.js';

const PADL = 52, PADR = 14, GAP = 8, SPAN = 200;
const H = { overview: 20, comp: 130, nodes: 150, axis: 20 };
const MONO = '10px ui-monospace, Menlo, monospace';
const MANY_VALUES = 24;

const vp = { follow: true, lo: 0, hover: null, args: null, overview: null, drag: null };
let seek = () => {};
export function useSeek(fn) { seek = fn; }
export function resetSvViewport() { Object.assign(vp, { follow: true, lo: 0, hover: null, drag: null }); }

const total = () => S.trace.rounds.length;

function domain() {
  const t = total();
  if (t <= SPAN) return [0, Math.max(t, 10)];
  const lo = vp.follow ? t - SPAN : Math.min(Math.max(vp.lo, 0), t - SPAN);
  return [lo, lo + SPAN];
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
  return { g, W };
}

const word = (label, value, cls = '') => `<span class="ro"><span class="roK">${label}</span> <b class="${cls}">${value}</b></span>`;

// Agreement is words in the readout: every holder on one value, nobody
// holding one, or neither.
export function agreementAt(t) {
  const vals = new Set(statesAt(t).filter(v => v !== null));
  if (vals.size === 0) return { text: 'all ⊥ (death spiral)', warn: true };
  if (vals.size === 1) return { text: 'agreement on ' + [...vals][0], warn: false };
  return { text: 'no agreement yet', warn: false };
}

function countsAt(t) {
  const counts = new Map();
  let holders = 0;
  for (const v of statesAt(t)) if (v !== null) { holders++; counts.set(v, (counts.get(v) || 0) + 1); }
  return { counts, holders };
}

function cursor() { return vp.hover !== null ? vp.hover : S.t; }

function renderReadout() {
  const t = cursor();
  const tr = S.trace;
  const info = roundInfo(t);
  const { counts, holders } = countsAt(t);
  const ag = agreementAt(t);
  $('svReadout').innerHTML = [
    word('round', `${t} / ${tr.rounds.length}`),
    word('value holders', `${holders} / ${tr.n}`),
    word('undecided ⊥', tr.n - holders),
    word('blocked', info ? info.blocked.length : '0 (initial)', info && info.blocked.length ? 'cRed' : ''),
    word('distinct values', counts.size),
    word('status', ag.text, ag.warn ? 'cRed' : ''),
  ].join('');
  const top = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8);
  $('svLegend').innerHTML = top.map(([v, c]) => `<span style="color:${colorOf(+v)}">value ${v} · ${c}</span>`).join(' · ') +
    (counts.size > 8 ? ` · + ${counts.size - 8} more values` : '') + (tr.n - holders ? ` · <span class="lgGrey">⊥ ${tr.n - holders}</span>` : '') +
    ' · <span class="lgRed">blocked strip</span>';
}

export function renderSvViewport() {
  if (!S.trace) { return; }
  const tr = S.trace;
  const [d0, d1] = domain();
  $('svLive').classList.toggle('hidden', vp.follow);
  $('svLive').textContent = S.live ? '↦ back to live' : '↦ to the end';
  drawOverview();
  drawLanes(d0, d1);
  renderReadout();
}

function drawOverview() {
  const P = prep($('svOverview'), H.overview);
  if (!P) return;
  const { g, W } = P;
  const tr = S.trace, T = total();
  const colW = (W - PADL - PADR) / Math.max(T, 1);
  const x = t => PADL + t * colW;
  g.fillStyle = '#14181e'; g.fillRect(PADL, 0, W - PADL - PADR, H.overview);
  const comp = S.series.comp;
  for (let t = 1; t <= T; t++) {
    const hold = (tr.n - comp[t].bot) / tr.n;
    g.fillStyle = BOT; g.fillRect(x(t - 1), 0, colW + 0.5, H.overview);
    g.fillStyle = '#3dd6c4'; g.fillRect(x(t - 1), H.overview * (1 - hold), colW + 0.5, H.overview * hold);
  }
  g.fillStyle = '#565f6b'; g.font = MONO; g.textAlign = 'right'; g.textBaseline = 'middle';
  g.fillText('run', PADL - 8, H.overview / 2);
  const [d0, d1] = domain();
  g.strokeStyle = '#eef2f6'; g.lineWidth = 1.5;
  g.strokeRect(x(d0) + 0.5, 0.5, Math.max(4, x(Math.min(d1, T)) - x(d0)) - 1, H.overview - 1);
  vp.overview = { x, T, W };
}

function drawLanes(d0, d1) {
  const tr = S.trace, T = total();
  const height = 4 + H.comp + GAP + H.nodes + 4 + H.axis;
  const P = prep($('svCanvas'), height);
  if (!P) return;
  const { g, W } = P;
  const plotW = W - PADL - PADR;
  const X = t => PADL + ((t - d0) / (d1 - d0)) * plotW;
  const first = Math.max(0, Math.floor(d0)), last = Math.min(T, Math.ceil(d1));
  const compY = 4, nodesY = 4 + H.comp + GAP, bottom = nodesY + H.nodes;
  const colW = plotW / (d1 - d0);
  vp.args = { X, plotW, d0, d1, T, W };

  g.save();
  g.beginPath(); g.rect(PADL, 0, plotW, height); g.clip();
  drawComposition(g, X, first, last, compY, tr);
  const note = drawNodes(g, X, first, last, nodesY, colW, tr);
  g.restore();

  g.font = MONO; g.fillStyle = '#6f7986'; g.textAlign = 'left'; g.textBaseline = 'top';
  g.fillText('value composition', PADL + 6, compY + 3);
  g.fillText(note, PADL + 6, nodesY + 3);

  g.fillStyle = '#565f6b'; g.textAlign = 'center';
  const step = Math.max(1, Math.ceil((d1 - d0) / 8 / 5) * 5);
  for (let t = Math.ceil(d0 / step) * step; t <= d1; t += step) g.fillText('r' + t, X(t), bottom + 6);

  const mark = (t, color, dash) => {
    if (t < d0 || t > d1) return;
    g.strokeStyle = color; g.lineWidth = 1; g.setLineDash(dash);
    g.beginPath(); g.moveTo(X(t) + 0.5, 2); g.lineTo(X(t) + 0.5, bottom); g.stroke(); g.setLineDash([]);
  };
  mark(S.t, '#eef2f6', [3, 3]);
  if (vp.hover !== null && vp.hover !== S.t) mark(vp.hover, '#e0af68', [3, 3]);
}

function drawComposition(g, X, first, last, y0, tr) {
  const sr = S.series, h = H.comp, n = tr.n;
  const strip = 7, ph = h - strip - 2;
  const Y = share => y0 + ph - share * ph;
  g.fillStyle = '#12161b'; g.fillRect(PADL, y0, vp.args.plotW, h);
  if (sr.values.length > MANY_VALUES) {
    g.fillStyle = '#3a8f86'; g.beginPath();
    for (let t = first; t <= last; t++) g.lineTo(X(t), Y((n - sr.comp[t].bot) / n));
    g.lineTo(X(last), Y(0)); g.lineTo(X(first), Y(0)); g.closePath(); g.fill();
  } else {
    const cum = [];
    for (let t = first; t <= last; t++) {
      const row = [0];
      for (let v = 0; v < sr.values.length; v++) row.push(row[v] + sr.comp[t].counts[v]);
      cum[t - first] = row;
    }
    for (let v = 0; v < sr.values.length; v++) {
      g.beginPath();
      for (let t = first; t <= last; t++) g.lineTo(X(t), Y(cum[t - first][v] / n));
      for (let t = last; t >= first; t--) g.lineTo(X(t), Y(cum[t - first][v + 1] / n));
      g.closePath();
      g.fillStyle = colorOf(sr.values[v]); g.globalAlpha = 0.9; g.fill(); g.globalAlpha = 1;
    }
  }
  g.fillStyle = BOT; g.globalAlpha = 0.5; g.beginPath();
  for (let t = first; t <= last; t++) g.lineTo(X(t), Y((n - sr.comp[t].bot) / n));
  g.lineTo(X(last), y0); g.lineTo(X(first), y0); g.closePath(); g.fill(); g.globalAlpha = 1;
  for (let t = Math.max(first, 1); t <= last; t++) {
    const share = sr.comp[t].blocked / n;
    if (share <= 0) continue;
    g.fillStyle = BLOCKED; g.globalAlpha = Math.min(0.25 + share * 2, 1);
    g.fillRect(X(t - 0.5), y0 + ph + 2, Math.max(1, X(t + 0.5) - X(t - 0.5)), strip - 2);
    g.globalAlpha = 1;
  }
}

// rows = nodes grouped by initial value; with more nodes than rows it shows
// every k-th node and says so
function drawNodes(g, X, first, last, y0, colW, tr) {
  const h = H.nodes;
  g.fillStyle = '#12161b'; g.fillRect(PADL, y0, vp.args.plotW, h);
  const maxRows = Math.max(Math.floor(h), 100);
  const stepN = Math.max(1, Math.ceil(tr.n / Math.min(maxRows, h)));
  const rowsShown = Math.ceil(tr.n / stepN);
  const ch = h / rowsShown;
  for (let t = first; t <= last; t++) {
    const st = statesAt(t);
    const blockedSet = t >= 1 ? new Set(roundInfo(t).blocked) : null;
    const x = X(t - 0.5);
    for (let r = 0; r < rowsShown; r++) {
      const node = S.rowOrder[r * stepN];
      const v = st[node];
      g.fillStyle = colorOf(v); g.globalAlpha = v === null ? 0.35 : 0.95;
      g.fillRect(x, y0 + r * ch, Math.ceil(colW), Math.ceil(ch));
      g.globalAlpha = 1;
      if (blockedSet && blockedSet.has(node)) { g.fillStyle = BLOCKED; g.fillRect(x, y0 + r * ch, Math.max(1.5, colW * 0.25), Math.ceil(ch)); }
    }
  }
  return stepN > 1 ? `per-node timeline · showing every ${stepN}. node of ${tr.n} · red notch = blocked` : 'per-node timeline · rows = nodes grouped by initial value · red notch = blocked';
}

/* ---------- interaction ---------- */

const redraw = () => { if (S.trace) renderSvViewport(); };
function roundAt(ev) {
  const a = vp.args;
  const rect = $('svCanvas').getBoundingClientRect();
  const r = Math.round(a.d0 + ((ev.clientX - rect.left - PADL) / a.plotW) * (a.d1 - a.d0));
  return Math.max(0, Math.min(a.T, r));
}
function pan(delta) {
  if (!S.trace || total() <= SPAN) return;
  const [d0] = domain();
  const lo = Math.min(Math.max(d0 + delta, 0), total() - SPAN);
  vp.lo = lo;
  vp.follow = lo >= total() - SPAN - 0.5;
  redraw();
}

$('svCanvas').addEventListener('mousemove', ev => {
  const a = vp.args;
  if (!a) return;
  if (vp.drag) {
    pan(-((ev.clientX - vp.drag.x) / a.plotW) * (a.d1 - a.d0));
    vp.drag.x = ev.clientX; vp.drag.moved = true;
    return;
  }
  vp.hover = roundAt(ev);
  redraw();
  const comp = S.series.comp[vp.hover];
  showTip(`round ${vp.hover} · ${S.trace.n - comp.bot}/${S.trace.n} hold a value · ${comp.blocked} blocked<br>click to move the grid here`, ev.clientX, ev.clientY);
});
$('svCanvas').addEventListener('mouseleave', () => { vp.hover = null; vp.drag = null; hideTip(); redraw(); });
$('svCanvas').addEventListener('mousedown', ev => { vp.drag = { x: ev.clientX, moved: false }; });
window.addEventListener('mouseup', () => { vp.drag = null; });
$('svCanvas').addEventListener('wheel', ev => {
  const d = Math.abs(ev.deltaX) > Math.abs(ev.deltaY) ? ev.deltaX : ev.shiftKey ? ev.deltaY : 0;
  if (!d || !vp.args) return;
  ev.preventDefault();
  pan((d / vp.args.plotW) * (vp.args.d1 - vp.args.d0));
}, { passive: false });
$('svCanvas').addEventListener('click', ev => {
  if (vp.drag && vp.drag.moved) return;
  if (!vp.args) return;
  seek(roundAt(ev));
});

function seekOverview(ev) {
  const o = vp.overview;
  if (!o || !S.trace || total() <= SPAN) return;
  const rect = $('svOverview').getBoundingClientRect();
  const r = ((ev.clientX - rect.left - PADL) / (o.W - PADL - PADR)) * o.T;
  const lo = Math.min(Math.max(r - SPAN / 2, 0), total() - SPAN);
  vp.lo = lo; vp.follow = lo >= total() - SPAN - 0.5;
  redraw();
}
let overviewDrag = false;
$('svOverview').addEventListener('mousedown', ev => { overviewDrag = true; seekOverview(ev); });
window.addEventListener('mousemove', ev => { if (overviewDrag) seekOverview(ev); });
window.addEventListener('mouseup', () => { overviewDrag = false; });
$('svLive').addEventListener('click', () => { vp.follow = true; if (S.live) seek(total()); redraw(); });
window.addEventListener('resize', redraw);
