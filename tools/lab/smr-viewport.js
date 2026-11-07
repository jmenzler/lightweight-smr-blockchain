// Viewport timeline: one shared x-axis of rounds, a fixed span that scrolls
// sideways, an overview strip of the whole run, and lanes for server health,
// command spread, command backlog and the committed chain. Canvas, because a
// long run redraws every round. Every plotted value comes from the report via
// smr-derive.js; the lane code only maps it to pixels.
import { S, $ } from './state.js';
import { derive, programBeta, chainBlocks, timelineEvents, phaseBands, spreadStats, neverCommitted, recRows, terminalText } from './smr-derive.js';
import { showTip, hideTip, escapeHtml } from './tooltip.js';

const PADL = 52, PADR = 52, GAP = 8;
const HEIGHTS = { overview: 20, health: 118, spread: 92, commands: 104, chain: 50, axis: 20 };
const MIN_SPAN = 100, EXTENDED_SPAN = 200, WINDOWS_IN_SPAN = 10;
const TEAL = '#3dd6c4', AMBER = '#e0af68', RED = '#f7768e', GREY = '#4b5563', PURPLE = '#bb9af7';
const MONO = '10px ui-monospace, Menlo, monospace';

const vp = { follow: true, lo: 0, hover: null, pinned: null, hits: [], layout: null, drag: null, rep: null };
const handlers = { openBlock: () => {}, openSpread: () => {} };
export function useViewportHandlers(h) { Object.assign(handlers, h); }

export function resetViewport() {
  Object.assign(vp, { follow: true, lo: 0, hover: null, pinned: null, hits: [], drag: null });
}

const ctxOf = () => S.smr.ctx;
const spanOf = c => (c.proto === 'extended' || !c.T ? EXTENDED_SPAN : Math.max(WINDOWS_IN_SPAN * c.T, MIN_SPAN));

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
  const [d0, d1] = domain(rep.metrics.length, spanOf(c));
  return [d0, d1];
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

function lanesFor(c) {
  const lanes = [['health', HEIGHTS.health], ['spread', HEIGHTS.spread], ['commands', HEIGHTS.commands]];
  if (c.proto === 'recovery' || c.proto === 'compact') lanes.push(['chain', HEIGHTS.chain]);
  let y = 4;
  const placed = {};
  for (const [key, h] of lanes) { placed[key] = { y, h }; y += h + GAP; }
  return { placed, total: y - GAP + HEIGHTS.axis + 4 };
}

const nice = v => {
  if (v <= 5) return Math.max(1, Math.ceil(v));
  const p = 10 ** Math.floor(Math.log10(v));
  return Math.ceil(v / p) * p;
};

export function renderViewport(rep) {
  vp.rep = rep;
  const c = ctxOf();
  if (!c || !rep.metrics.length) { $('vpWrap').classList.add('hidden'); return; }
  $('vpWrap').classList.remove('hidden');
  $('vpLegend').innerHTML = (c.proto === 'recovery'
    ? '<span class="lgTeal">no-reset</span> · <span class="lgAmber">reset</span> · <span class="lgGrey">⊥</span>'
    : '<span class="lgTeal">holds a log</span> · <span class="lgGrey">⊥</span>') + ' · <span class="lgRed">blocked</span>';
  drawOverview(rep, c);
  drawLanes(rep, c);
  $('vpLive').classList.toggle('hidden', vp.follow);
  renderReadout(rep, c);
}

function totalOf(rep) { return rep.metrics.length; }

function healthShares(rep, c, i) {
  const rows = recRows(rep);
  const n = c.n;
  if (c.proto === 'recovery' && rows) return { a: rows[i].noreset / n, b: rows[i].reset / n, g: rows[i].bot_r / n };
  const m = rep.metrics[i];
  return { a: m.nonbot_logs / n, b: 0, g: (n - m.nonbot_logs) / n };
}

function drawOverview(rep, c) {
  const P = prep($('vpOverview'), HEIGHTS.overview);
  if (!P) return;
  const { g, W, H } = P;
  const total = totalOf(rep);
  const x = r => PADL + (r / total) * (W - PADL - PADR);
  const colW = Math.max(1, (W - PADL - PADR) / total);
  g.fillStyle = '#14181e';
  g.fillRect(PADL, 0, W - PADL - PADR, H);
  for (let i = 0; i < total; i++) {
    const s = healthShares(rep, c, i);
    let y = H;
    for (const [v, col] of [[s.a, TEAL], [s.b, AMBER], [s.g, GREY]]) {
      g.fillStyle = col;
      g.fillRect(x(i), y - v * H, colW + 0.5, v * H);
      y -= v * H;
    }
  }
  g.fillStyle = '#565f6b'; g.font = MONO; g.textAlign = 'right'; g.textBaseline = 'middle';
  g.fillText('run', PADL - 8, H / 2);
  const [d0, d1] = domain(total, spanOf(c));
  g.strokeStyle = '#eef2f6'; g.lineWidth = 1.5;
  g.strokeRect(x(d0) + 0.5, 0.5, Math.max(4, x(Math.min(d1, total)) - x(d0)) - 1, H - 1);
  vp.overview = { x, total, W };
}

function drawLanes(rep, c) {
  const { placed, total: height } = lanesFor(c);
  vp.layout = placed;
  const P = prep($('vpCanvas'), height);
  if (!P) return;
  const { g, W } = P;
  const nRounds = totalOf(rep);
  const span = spanOf(c);
  const [d0, d1] = domain(nRounds, span);
  const plotW = W - PADL - PADR;
  const X = r => PADL + ((r - d0) / (d1 - d0)) * plotW;
  const first = Math.max(1, Math.floor(d0));
  const last = Math.min(nRounds, Math.ceil(d1));
  const D = derive(rep, c.proto);
  const events = timelineEvents(rep, c, S.smr.safetyRound ?? null);
  const args = { g, rep, c, X, d0, d1, first, last, D, W, events, plotW };
  vp.args = args;
  vp.hits = [];

  const gridBottom = height - HEIGHTS.axis - 4;
  g.lineWidth = 1;
  if (c.T) {
    g.strokeStyle = '#1a1f27';
    for (let b = Math.ceil(d0 / c.T) * c.T; b <= d1; b += c.T) {
      g.beginPath(); g.moveTo(X(b) + 0.5, 4); g.lineTo(X(b) + 0.5, gridBottom); g.stroke();
    }
  }
  drawHealth(args, placed.health);
  if (placed.spread) drawSpread(args, placed.spread);
  if (placed.commands) drawCommands(args, placed.commands);
  if (placed.chain) drawChain(args, placed.chain);
  drawEventMarkers(args, gridBottom);
  drawAxis(args, height);
  drawCursor(args, gridBottom);
}

function laneLabel(g, text, y, x = PADL + 6, color = '#6f7986') {
  g.font = MONO; g.fillStyle = color; g.textAlign = 'left'; g.textBaseline = 'top';
  g.fillText(text, x, y + 3);
}

/* ---------- lane 1: server health ---------- */

const BAND_COLOR = { collapsed: ['rgba(75,85,99,0.45)', 'collapsed'], recovering: ['rgba(224,175,104,0.35)', 'recovering'], recovered: ['rgba(61,214,196,0.22)', 'recovered'] };

function drawHealth(a, { y, h }) {
  const { g, rep, c, X, d0, d1, first, last } = a;
  const Y = share => y + h - share * h;
  g.fillStyle = '#12161b'; g.fillRect(PADL, y, a.plotW, h);
  const layers = c.proto === 'recovery'
    ? [['a', TEAL], ['b', AMBER], ['g', GREY]] : [['a', TEAL], ['g', GREY]];
  const base = new Array(last - first + 1).fill(0);
  for (const [key, col] of layers) {
    g.fillStyle = col;
    g.beginPath();
    for (let r = first; r <= last; r++) g.lineTo(X(r), Y(base[r - first]));
    for (let r = last; r >= first; r--) {
      base[r - first] += healthShares(rep, c, r - 1)[key];
      g.lineTo(X(r), Y(base[r - first]));
    }
    g.closePath(); g.fill();
  }
  // blocked share as a red line over the stack, only where something is blocked
  g.strokeStyle = RED; g.lineWidth = 1.4;
  g.beginPath();
  let pen = false;
  for (let r = first; r <= last; r++) {
    const blocked = rep.metrics[r - 1].blocked;
    if (!blocked && !(r > first && rep.metrics[r - 2].blocked)) { pen = false; continue; }
    const yy = Y(blocked / c.n);
    pen ? g.lineTo(X(r), yy) : g.moveTo(X(r), yy);
    pen = true;
  }
  g.stroke();
  // the plan ahead of the newest round, as a faint line
  if (S.smr.live && S.smr.program && last < d1) {
    g.strokeStyle = 'rgba(247,118,142,0.55)'; g.lineWidth = 1.2; g.setLineDash([3, 3]);
    g.beginPath();
    for (let r = last + 1; r <= Math.floor(d1); r++) {
      const yy = Y((programBeta(S.smr.program, r) ?? 0) / 100);
      r === last + 1 ? g.moveTo(X(r), yy) : g.lineTo(X(r), yy);
    }
    g.stroke(); g.setLineDash([]);
  }
  // phase bands along the top edge
  for (const band of phaseBands(rep, c)) {
    if (band.to < d0 || band.from > d1) continue;
    const x0 = Math.max(PADL, X(band.from)), x1 = Math.min(PADL + a.plotW, X(band.to));
    if (x1 <= x0) continue;
    const [fill, label] = BAND_COLOR[band.kind];
    g.fillStyle = fill; g.fillRect(x0, y, x1 - x0, 13);
    g.font = MONO; g.fillStyle = '#c9d2dd'; g.textAlign = 'left'; g.textBaseline = 'top';
    if (x1 - x0 > label.length * 6.4 + 8) g.fillText(label, x0 + 4, y + 1);
  }
  laneLabel(g, 'servers', y + 14);
  g.textAlign = 'right'; g.textBaseline = 'middle'; g.fillStyle = '#565f6b';
  g.fillText('100%', PADL - 6, y + 6); g.fillText('0', PADL - 6, y + h - 5);
}

/* ---------- lane 2: command spread ---------- */

// The lane is about the spreading phase: a curve ends at all_logs_round, or at
// its peak when it has none, because it falls back to 0 once the entry commits.
function spreadingPhase(cmd) {
  const pts = cmd.spread;
  if (cmd.all_logs_round != null) return pts.filter(p => p.round <= cmd.all_logs_round);
  let peak = 0;
  pts.forEach((p, j) => { if (p.useful_holders > pts[peak].useful_holders) peak = j; });
  return pts.slice(0, peak + 1);
}

function drawSpread(a, { y, h }) {
  const { g, rep, c, X, d0, d1 } = a;
  g.fillStyle = '#12161b'; g.fillRect(PADL, y, a.plotW, h);
  const Y = share => y + h - 4 - share * (h - 18);
  const visible = [];
  rep.commands.forEach((cmd, i) => {
    const pts = cmd.spread;
    if (!pts || !pts.length) return;
    if (pts[pts.length - 1].round < d0 || pts[0].round > d1) return;
    visible.push(i);
  });
  g.save();
  g.beginPath(); g.rect(PADL, y, a.plotW, h); g.clip();
  visible.forEach((i, rank) => {
    const pts = rep.commands[i].spread;
    const age = visible.length - 1 - rank;
    const sel = S.smr.sel === i;
    g.globalAlpha = sel ? 1 : age < 24 ? 0.95 - age * 0.032 : 0.14;
    g.strokeStyle = sel ? '#ffffff' : TEAL;
    g.lineWidth = sel ? 2.2 : age === 0 ? 1.8 : 1.1;
    g.beginPath();
    const shown = spreadingPhase(rep.commands[i]);
    shown.forEach((p, j) => {
      const yy = Y(p.useful_total ? p.useful_holders / p.useful_total : 0);
      j === 0 ? g.moveTo(X(p.round), yy) : g.lineTo(X(p.round), yy);
    });
    g.stroke();
  });
  g.restore();
  g.globalAlpha = 1;
  const inView = rep.commands.filter(cmd => cmd.delivered_round !== null && cmd.delivered_round >= d0 && cmd.delivered_round <= d1);
  const st = spreadStats(inView, c.n);
  vp.spreadStats = st;
  const label = st.measured
    ? `spread · ${st.median}r median · ${st.p90}r p90 · ${st.multiple.toFixed(1)}× log₂n` + (st.spreading ? ` · ${st.spreading} still spreading` : '')
    : `spread · ${st.spreading ? `${st.spreading} still spreading, none at every server yet` : 'no command delivered in view'}`;
  laneLabel(g, label, y);
  const btn = $('vpSpreadBtn');
  btn.style.top = (y + 1) + 'px';
}

/* ---------- lane 3: commands ---------- */

function drawCommands(a, { y, h }) {
  const { g, c, X, d0, d1, first, last, D, events } = a;
  g.fillStyle = '#12161b'; g.fillRect(PADL, y, a.plotW, h);
  let maxP = 1, maxA = 1;
  for (let r = first; r <= last; r++) { maxP = Math.max(maxP, D.pending[r]); maxA = Math.max(maxA, D.oldest[r]); }
  if (c.proto === 'recovery' && c.tw) maxA = Math.max(maxA, 2 * c.tw);
  maxP = nice(maxP); maxA = nice(maxA);
  const top = y + 14, bottom = y + h - 4;
  const YP = v => bottom - (v / maxP) * (bottom - top);
  const YA = v => bottom - (v / maxA) * (bottom - top);
  g.fillStyle = 'rgba(187,154,247,0.22)';
  g.beginPath(); g.moveTo(X(first), bottom);
  for (let r = first; r <= last; r++) g.lineTo(X(r), YP(D.pending[r]));
  g.lineTo(X(last), bottom); g.closePath(); g.fill();
  g.strokeStyle = PURPLE; g.lineWidth = 1.2;
  g.beginPath();
  for (let r = first; r <= last; r++) r === first ? g.moveTo(X(r), YP(D.pending[r])) : g.lineTo(X(r), YP(D.pending[r]));
  g.stroke();
  g.strokeStyle = AMBER; g.lineWidth = 1.1; g.setLineDash([4, 3]);
  g.beginPath();
  for (let r = first; r <= last; r++) r === first ? g.moveTo(X(r), YA(D.oldest[r])) : g.lineTo(X(r), YA(D.oldest[r]));
  g.stroke(); g.setLineDash([]);
  if (c.proto === 'recovery' && c.tw) {
    const yy = YA(2 * c.tw);
    g.strokeStyle = 'rgba(224,175,104,0.35)'; g.setLineDash([2, 4]);
    g.beginPath(); g.moveTo(PADL, yy); g.lineTo(PADL + a.plotW, yy); g.stroke(); g.setLineDash([]);
    g.font = MONO; g.fillStyle = 'rgba(224,175,104,0.7)'; g.textAlign = 'right'; g.textBaseline = 'bottom';
    g.fillText('2T: normal commit latency', PADL + a.plotW - 4, yy - 2);
  }
  for (const ev of events) {
    if (ev.kind !== 'rollback' || ev.round < d0 || ev.round > d1) continue;
    g.strokeStyle = RED; g.lineWidth = 1;
    g.beginPath(); g.moveTo(X(ev.round) + 0.5, top); g.lineTo(X(ev.round) + 0.5, bottom); g.stroke();
  }
  laneLabel(g, 'backlog', y, PADL + 6, PURPLE);
  laneLabel(g, 'oldest waiting', y, PADL + 62, AMBER);
  g.font = MONO; g.textBaseline = 'middle';
  g.textAlign = 'right'; g.fillStyle = PURPLE;
  g.fillText(String(maxP), PADL - 6, top); g.fillText('0', PADL - 6, bottom);
  g.textAlign = 'left'; g.fillStyle = AMBER;
  g.fillText(maxA + 'r', PADL + a.plotW + 6, top); g.fillText('0r', PADL + a.plotW + 6, bottom);
}

/* ---------- lane 4: chain ---------- */

const BLOCK_STYLE = {
  committed: ['#16302d', TEAL], drain: ['#241f14', AMBER], rollback: ['#241f14', '#8a6a2c'],
  empty: ['#14181e', '#2c343e'],
};

function drawChain(a, { y, h }) {
  const { g, rep, c, X, d0, d1 } = a;
  const blocks = chainBlocks(rep, c.proto, c.T);
  const next = { m: blocks.length + 1, round: (blocks.length + 1) * c.T, state: 'next', label: c.proto === 'recovery' ? `b${(blocks.length + 1) * c.T}` : '' };
  for (const b of [...blocks, next]) {
    const x0 = X(b.round - c.T), x1 = X(b.round);
    if (x1 < PADL || x0 > PADL + a.plotW) continue;
    const bx = Math.max(PADL, x0) + 1, bw = Math.min(PADL + a.plotW, x1) - bx - 1;
    if (bw < 2) continue;
    const sel = S.smr.blockSel === b.m && b.state !== 'next';
    if (b.state === 'next') {
      g.strokeStyle = '#333b46'; g.setLineDash([4, 3]);
      g.strokeRect(bx + 0.5, y + 0.5, bw - 1, h - 1); g.setLineDash([]);
    } else {
      const [fill, edge] = BLOCK_STYLE[b.state];
      g.fillStyle = fill; g.fillRect(bx, y, bw, h);
      g.strokeStyle = sel ? '#eef2f6' : edge; g.lineWidth = sel ? 2 : 1;
      g.strokeRect(bx + 0.5, y + 0.5, bw - 1, h - 1);
    }
    if (bw > 38) {
      g.textAlign = 'center'; g.textBaseline = 'middle';
      g.font = MONO; g.fillStyle = '#6f7986';
      g.fillText(b.label, bx + bw / 2, y + 12);
      g.font = 'bold 12px ui-monospace, Menlo, monospace';
      const [, edge] = BLOCK_STYLE[b.state] || ['', '#565f6b'];
      g.fillStyle = b.state === 'next' ? '#565f6b' : edge;
      const big = b.state === 'rollback' ? '⟲' : b.state === 'next' ? 'next' : (c.proto === 'compact' ? `+${b.count}` : String(b.size));
      g.fillText(big, bx + bw / 2, y + 28);
      if (b.latency && bw > 64) {
        g.font = MONO; g.fillStyle = '#8b94a1';
        g.fillText(`⌀ ${Math.round(b.latency.avg)}r${b.state === 'drain' ? ' drain' : ''}`, bx + bw / 2, y + 42);
      } else if (b.state === 'drain' && bw > 64) {
        g.font = MONO; g.fillStyle = AMBER; g.fillText('drain', bx + bw / 2, y + 42);
      }
    }
    if (b.state !== 'next') vp.hits.push({ x: bx, y, w: bw, h, block: b });
  }
  g.font = MONO; g.fillStyle = '#6f7986'; g.textAlign = 'right'; g.textBaseline = 'middle';
  g.fillText('chain', PADL - 6, y + h / 2);
}

/* ---------- markers, axis, cursor ---------- */

const EVENT_COLOR = { rollback: RED, collapse: '#9ca3af', release: AMBER, confirmed: TEAL, checkpoint: '#5b6572', safety: RED };

function drawEventMarkers(a, bottom) {
  const { g, X, d0, d1, events } = a;
  let lastLabelX = -1e9;
  const top = 4;
  vp.marks = [];
  for (const ev of events) {
    if (ev.round < d0 || ev.round > d1) continue;
    const x = X(ev.round);
    const col = EVENT_COLOR[ev.kind] || '#8b94a1';
    if (ev.kind === 'rollback') {
      g.strokeStyle = col; g.lineWidth = 2;
      const hb = vp.layout.health;
      g.beginPath(); g.moveTo(x + 0.5, hb.y + hb.h - 12); g.lineTo(x + 0.5, hb.y + hb.h); g.stroke();
    } else if (ev.kind === 'checkpoint') {
      g.strokeStyle = col; g.lineWidth = 1;
      const hb = vp.layout.health;
      g.beginPath(); g.moveTo(x + 0.5, hb.y + hb.h - 6); g.lineTo(x + 0.5, hb.y + hb.h); g.stroke();
    } else {
      g.strokeStyle = col; g.lineWidth = 1; g.setLineDash([3, 3]);
      g.beginPath(); g.moveTo(x + 0.5, top); g.lineTo(x + 0.5, bottom); g.stroke(); g.setLineDash([]);
    }
    if (ev.label && x - lastLabelX > 58) {
      g.font = '10px ui-monospace, Menlo, monospace'; g.fillStyle = col; g.textAlign = 'left'; g.textBaseline = 'top';
      g.fillText(ev.label, x + 3, vp.layout.health.y + 16);
      lastLabelX = x;
    }
    vp.marks.push({ x, ev });
  }
}

function drawAxis(a, height) {
  const { g, X, d0, d1, c } = a;
  g.font = MONO; g.fillStyle = '#565f6b'; g.textAlign = 'center'; g.textBaseline = 'top';
  const step = Math.max(c.T || 1, Math.ceil((d1 - d0) / 8 / (c.T || 1)) * (c.T || 1));
  for (let r = Math.ceil(d0 / step) * step; r <= d1; r += step) g.fillText('r' + r, X(r), height - HEIGHTS.axis);
}

function cursorRound(rep) {
  if (vp.hover !== null) return vp.hover;
  if (vp.pinned !== null) return vp.pinned;
  return Math.min(rep.metrics.length, Math.ceil(vp.args ? vp.args.d1 : rep.metrics.length));
}

function drawCursor(a, bottom) {
  const { g, X, rep, d0, d1 } = a;
  const r = cursorRound(rep);
  if (r < d0 || r > d1) return;
  g.strokeStyle = vp.pinned === r ? '#eef2f6' : AMBER; g.lineWidth = 1; g.setLineDash([3, 3]);
  g.beginPath(); g.moveTo(X(r) + 0.5, 4); g.lineTo(X(r) + 0.5, bottom); g.stroke(); g.setLineDash([]);
}

/* ---------- readout line ---------- */

const word = (label, value, cls = '') => `<span class="ro"><span class="roK">${label}</span> <b class="${cls}">${value}</b></span>`;

function renderReadout(rep, c) {
  const r = cursorRound(rep);
  const m = rep.metrics[r - 1];
  if (!m) { $('vpReadout').innerHTML = ''; return; }
  const D = derive(rep, c.proto);
  const rows = recRows(rep);
  const parts = [word('round', r)];
  parts.push(word('blocked', Math.round((m.blocked / c.n) * 100) + '%', m.blocked ? 'cRed' : ''));
  if (c.proto === 'recovery' && rows) {
    const row = rows[r - 1];
    parts.push(word('servers', `${row.noreset} ok · ${row.reset} reset · ${row.bot_r} ⊥`));
  } else {
    parts.push(word('servers', `${m.nonbot_logs} with log · ${c.n - m.nonbot_logs} ⊥`));
  }
  parts.push(word('backlog', D.pending[r]));
  parts.push(word('oldest waiting', D.oldest[r] + ' rounds'));
  parts.push(word(c.proto === 'extended' ? 'in every log' : 'committed', D.committed[r]));
  parts.push(word('agreed prefix', `${m.lcp_len}/${Math.max(m.max_log_len, m.max_executed_len)}`));
  if (c.proto !== 'extended') parts.push(word('executed', `${m.min_executed_len}–${m.max_executed_len}`));
  const term = terminalText(rep);
  if (term) parts.push(`<span class="ro cRed"><b>${escapeHtml(term)}</b></span>`);
  const nc = neverCommitted(rep, c.proto);
  if (!S.smr.live && nc.total > 0) {
    const tip = `in flight ${nc.inFlight}` + (c.proto === 'extended' ? ` · stranded ${nc.stranded} (client stopped after delivery, no amplify receiver)` : '') + ` · dead ${nc.dead}`;
    parts.push(`<span class="ro" data-tip="${escapeHtml(tip)}"><span class="roK">never committed</span> <b class="cRed">${nc.total}</b></span>`);
  }
  if (!rep.safety_ok) {
    const at = S.smr.safetyRound ?? rep.terminal?.Halted?.round;
    parts.push(`<span class="ro cRed"><b>SAFETY VIOLATION${at ? ' from round ' + at : ' (round not recorded)'}</b></span>`);
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

$('vpCanvas').addEventListener('mousemove', ev => {
  const a = vp.args;
  if (!a) return;
  if (vp.drag) {
    const rect = $('vpCanvas').getBoundingClientRect();
    pan(-((ev.clientX - vp.drag.x) / a.plotW) * (a.d1 - a.d0));
    vp.drag.x = ev.clientX;
    vp.drag.moved = true;
    return;
  }
  const r = roundAtEvent(ev);
  vp.hover = r;
  redraw();
  const rect = $('vpCanvas').getBoundingClientRect();
  const x = ev.clientX - rect.left, y = ev.clientY - rect.top;
  const block = vp.hits.find(h => x >= h.x && x <= h.x + h.w && y >= h.y && y <= h.y + h.h);
  const mark = vp.marks && vp.marks.find(m => Math.abs(m.x - x) < 4 && y < vp.layout.health.y + vp.layout.health.h && m.ev.tip);
  if (block) showTip(blockTip(block.block, a.c), ev.clientX, ev.clientY);
  else if (mark) showTip(escapeHtml(mark.ev.tip), ev.clientX, ev.clientY);
  else hideTip();
});
$('vpCanvas').addEventListener('mouseleave', () => { vp.hover = null; vp.drag = null; hideTip(); redraw(); });
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
  const x = ev.clientX - rect.left, y = ev.clientY - rect.top;
  const hit = vp.hits.find(h => x >= h.x && x <= h.x + h.w && y >= h.y && y <= h.y + h.h);
  if (hit) {
    S.smr.blockSel = S.smr.blockSel === hit.block.m ? null : hit.block.m;
    handlers.openBlock(S.smr.blockSel === null ? null : hit.block);
    redraw();
    return;
  }
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

function blockTip(b, c) {
  const lat = b.latency;
  const lines = [c.proto === 'compact'
    ? `<b>${escapeHtml(b.label)}</b> · ${b.count} command${b.count === 1 ? '' : 's'} committed in this window`
    : `<b>${escapeHtml(b.label)}</b> · ${b.size} entr${b.size === 1 ? 'y' : 'ies'} committed`];
  if (b.state === 'rollback') lines.push(`${b.rb} server${b.rb === 1 ? '' : 's'} rolled back here; nothing committed.`);
  if (b.state === 'drain') lines.push('first batch after a rollback: the re-aged backlog landing at once');
  if (lat) lines.push(`submit→commit latency over ${lat.n} command${lat.n === 1 ? '' : 's'}: ⌀ ${lat.avg.toFixed(1)}r · median ${lat.median}r · max ${lat.max}r`);
  else if (b.size > 0 && c.proto !== 'compact') lines.push('no client commands in this block');
  lines.push('click to open');
  return lines.join('<br>');
}

window.addEventListener('resize', redraw);
