// keyboard, node selection on the grid, file opening
import { S, $ } from './state.js';
import { update, pos, setView } from './render.js';
import { loadTrace } from './trace.js';
import { looksLikeSpill, loadSpill } from './spill.js';
import { importSingleSpec } from './form.js';
import { runSpecBatch } from './smr.js';
import { openTrace, togglePlay, stepForward, stepBack } from './live.js';

window.addEventListener('keydown', e => {
  if (e.target.tagName === 'INPUT' || e.target.tagName === 'SELECT' || S.view !== 'single' || !S.sv.ctx) return;
  if (e.key === 'ArrowRight') { e.preventDefault(); stepForward(); }
  else if (e.key === 'ArrowLeft') { e.preventDefault(); stepBack(); }
  else if (e.key === ' ') { e.preventDefault(); togglePlay(); }
});

$('net').addEventListener('click', e => {
  if (!S.trace || !S.layout || S.layout.cell < 6) return;
  const rect = e.target.getBoundingClientRect();
  const x = e.clientX - rect.left, y = e.clientY - rect.top;
  let best = null, bestD = Infinity;
  for (let i = 0; i < S.trace.n; i++) {
    const p = pos(i, S.layout);
    const d = (p.x - x) ** 2 + (p.y - y) ** 2;
    if (d < bestD) { bestD = d; best = i; }
  }
  S.sel = bestD <= S.layout.cell * S.layout.cell * 0.4 ? best : null;
  update();
});

/* load / drag-drop */
// A spill is JSONL, so a whole-file JSON.parse throws on it. Sniff the first
// line before parsing; after that a file is a trace, an SMR scenario spec
// (proto.kind) or a single-value spec (init).
const ingest = text => {
  if (looksLikeSpill(text)) { loadSpill(text); return; }
  const parsed = JSON.parse(text);
  if (Array.isArray(parsed.rounds)) { setView('single'); openTrace(parsed); }
  else if (parsed.proto && typeof parsed.proto.kind === 'string') { setView('smr'); runSpecBatch(parsed); }
  else if (parsed.init) { setView('single'); importSingleSpec(parsed); }
  else console.warn('not a trace, spill or scenario spec');
};

$('loadBtn').addEventListener('click', () => {
  const inp = document.createElement('input');
  inp.type = 'file'; inp.accept = '.json,.jsonl,application/json';
  inp.onchange = () => {
    const f = inp.files && inp.files[0];
    if (!f) return;
    f.text().then(t => ingest(t)).catch(e => console.warn(e));
  };
  inp.click();
});
document.addEventListener('dragover', e => { e.preventDefault(); $('dropHint').classList.add('on'); });
document.addEventListener('dragleave', e => { if (e.relatedTarget === null) $('dropHint').classList.remove('on'); });
document.addEventListener('drop', e => {
  e.preventDefault();
  $('dropHint').classList.remove('on');
  const f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
  if (f) f.text().then(t => ingest(t)).catch(err => console.warn(err));
});
