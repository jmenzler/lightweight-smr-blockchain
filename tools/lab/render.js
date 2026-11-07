// views + the single-value session page: setView / update / node grid
import { S, $, colorOf, BLOCKED, statesAt, roundInfo } from './state.js';
import { renderSvViewport } from './sv-viewport.js';

export function setView(v) {
  S.view = v;
  document.querySelectorAll('#modeTabs .mt').forEach(b => b.classList.toggle('on', b.dataset.mode === v));
  document.querySelectorAll('.view').forEach(el => el.classList.toggle('on', el.id === 'view-' + v));
  $('app').dataset.view = v;
  draw();
}

export function update() {
  const tr = S.trace;
  if (!tr) { draw(); return; }
  const states = statesAt(S.t);
  const info = roundInfo(S.t);
  let holders = 0;
  for (const v of states) if (v !== null) holders++;
  $('svCounts').textContent = `${holders} with value · ${tr.n - holders} ⊥ · ${info ? info.blocked.length : 0} blocked · at r${S.t}`;

  const insp = $('inspector');
  if (S.sel !== null) {
    insp.classList.remove('hidden');
    const s = S.sel;
    $('selId').textContent = '#' + s;
    $('selState').textContent = states[s] === null ? '⊥' : 'value ' + states[s];
    const isBlocked = info ? info.blocked.includes(s) : false;
    $('selBlocked').textContent = info ? (isBlocked ? 'yes' : 'no') : '–';
    const hasT = info && info.targets && info.targets.length === tr.n;
    if (hasT) {
      $('selTargets').textContent = info.targets[s].join(', ');
      const by = [];
      for (let q = 0; q < tr.n; q++) if (q !== s && info.targets[q].includes(s)) by.push(q);
      $('selSamplers').textContent = by.length ? by.join(', ') : '—';
    } else {
      $('selTargets').textContent = info ? 'not recorded (large n)' : '– (initial)';
      $('selSamplers').textContent = '–';
    }
  } else insp.classList.add('hidden');

  draw();
}

/* ---------- canvases ---------- */
function prep(id) {
  const cv = $(id);
  if (!cv) return null;
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth, h = cv.clientHeight;
  if (w === 0 || h === 0) return null;
  if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); }
  const ctx = cv.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  return { ctx, w, h };
}

function layout(w, h, n) {
  const cols = Math.ceil(Math.sqrt(n * (w / Math.max(h, 1))));
  const rows = Math.ceil(n / cols);
  const cell = Math.min(w / cols, h / rows);
  return { cols, cell, ox: (w - cols * cell) / 2 + cell / 2, oy: (h - rows * cell) / 2 + cell / 2, r: Math.max(1, Math.min(cell * 0.3, 9)) };
}
export const pos = (i, L) => ({ x: L.ox + (i % L.cols) * L.cell, y: L.oy + Math.floor(i / L.cols) * L.cell });

export function draw() { drawNet(); renderSvViewport(); }

function drawNet() {
  const P = prep('net');
  if (!P) return;
  const { ctx, w, h } = P;
  const tr = S.trace;
  if (!tr) return;
  const L = layout(w - 24, h - 24, tr.n);
  L.ox += 12; L.oy += 12;
  S.layout = L;
  const pixelMode = L.cell < 6;
  const states = statesAt(S.t);
  const info = roundInfo(S.t);
  const hasTargets = info && info.targets && info.targets.length === tr.n;
  const blockedSet = new Set(info ? info.blocked : []);

  if (hasTargets && S.edgeMode === 'all' && !pixelMode) {
    ctx.globalAlpha = 0.05; ctx.strokeStyle = '#8b94a1'; ctx.lineWidth = 1;
    for (let i = 0; i < tr.n; i++) {
      const pi = pos(i, L);
      for (const t of info.targets[i]) {
        const pt = pos(t, L);
        ctx.beginPath(); ctx.moveTo(pi.x, pi.y); ctx.lineTo(pt.x, pt.y); ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;
  }
  if (hasTargets && S.edgeMode !== 'none' && S.sel !== null && !pixelMode) {
    const s = S.sel, ps = pos(s, L);
    ctx.lineWidth = 1.5; ctx.strokeStyle = '#269c86'; ctx.globalAlpha = 0.95;
    for (const o of info.targets[s]) { const po = pos(o, L); ctx.beginPath(); ctx.moveTo(ps.x, ps.y); ctx.lineTo(po.x, po.y); ctx.stroke(); }
    ctx.strokeStyle = '#c05a9e'; ctx.setLineDash([4, 3]);
    for (let q = 0; q < tr.n; q++) {
      if (q !== s && info.targets[q].includes(s)) { const pq = pos(q, L); ctx.beginPath(); ctx.moveTo(pq.x, pq.y); ctx.lineTo(ps.x, ps.y); ctx.stroke(); }
    }
    ctx.setLineDash([]); ctx.globalAlpha = 1;
  }

  if (pixelMode) {
    const side = Math.max(1, L.cell * 0.92), half = side / 2;
    for (let k = 0; k < tr.n; k++) {
      const p = pos(k, L), v = states[k];
      ctx.fillStyle = colorOf(v);
      ctx.globalAlpha = v === null ? 0.4 : 1;
      ctx.fillRect(p.x - half, p.y - half, side, side);
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = '#6f7986'; ctx.font = '11px ui-monospace';
    ctx.fillText(`pixel mode (n = ${tr.n}) — rings, selection and edges need fewer nodes`, 14, h - 10);
  } else {
    for (let k = 0; k < tr.n; k++) {
      const p = pos(k, L), v = states[k];
      ctx.beginPath(); ctx.arc(p.x, p.y, L.r, 0, 7);
      ctx.fillStyle = colorOf(v);
      ctx.globalAlpha = v === null ? 0.45 : 1;
      ctx.fill(); ctx.globalAlpha = 1;
      if (blockedSet.has(k)) { ctx.lineWidth = 2; ctx.strokeStyle = BLOCKED; ctx.beginPath(); ctx.arc(p.x, p.y, L.r + 2.5, 0, 7); ctx.stroke(); }
      if (S.sel === k) { ctx.lineWidth = 2; ctx.strokeStyle = '#eef2f6'; ctx.beginPath(); ctx.arc(p.x, p.y, L.r + 5, 0, 7); ctx.stroke(); }
    }
  }
}
