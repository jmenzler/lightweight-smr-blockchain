// trace loading + derived series
import { S, $ } from './state.js';
import { update } from './render.js';

/* ---------- trace handling ---------- */
export function loadTrace(tr) {
  if (!tr || !Array.isArray(tr.initial) || !Array.isArray(tr.rounds)) { console.warn('not a trace file'); return; }
  const seen = {};
  const collect = st => { for (const v of st) if (v !== null) seen[v] = true; };
  collect(tr.initial);
  for (const r of tr.rounds) collect(r.states);
  const values = Object.keys(seen).map(Number).sort((a, b) => a - b);
  const vIndex = {};
  values.forEach((v, i) => vIndex[v] = i);
  const comp = [];
  for (let t = 0; t <= tr.rounds.length; t++) {
    const st = t === 0 ? tr.initial : tr.rounds[t - 1].states;
    const counts = new Array(values.length).fill(0);
    let bot = 0;
    for (const v of st) { if (v === null) bot++; else counts[vIndex[v]]++; }
    comp.push({ counts, bot, blocked: t === 0 ? 0 : tr.rounds[t - 1].blocked.length });
  }
  S.series = { values, vIndex, comp };
  S.rowOrder = [...Array(tr.n).keys()].sort((a, b) => {
    const va = tr.initial[a] === null ? 1e9 : tr.initial[a];
    const vb = tr.initial[b] === null ? 1e9 : tr.initial[b];
    return va - vb || a - b;
  });
  S.trace = tr; S.t = 0; S.sel = null; S.playing = false;
  update();
}
