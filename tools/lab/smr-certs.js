// Committed sequence + Merkle forest (design: hanging forest, spec
// 2026-07-23-lab-gui-revamp-design.md). Renders LiveSmr.certs_json() verbatim:
// block row = the full committed sequence (horizontal scroll owns history,
// view follows the newest block), trees hang from the blocks (structure
// derived from peaks — heights + starts + roots come from the engine;
// internal hashes are not exposed and none are invented). Big m falls back
// to the peaks skyline.
//
// Two feeds share every painter. Compact streams certs_json and gets the §5
// certificate layer on top; recovery streams chain_json, which carries the
// same forest over Alg 6's committed sequence but no certificates — that
// layer is not constructed for Alg 6, so its UI is suppressed rather than
// faked (`certsMode`).
import { $, S, PALETTE, AUTO_CLIENT_BASE } from './state.js';
import { post } from './engine.js';

let snap = null;
let selPos = null; // selected committed position (from a client cert row)
let anim = [];
// on-demand verify-everywhere tallies (button-refreshed, round-stamped):
// tallies.map[client][sn] = {accepted, covered} | null (no cert constructible)
let tallies = null;
// 'compact' = §5 as the paper proves it · 'recovery' = the RQ11 construction
// over Alg 6 · 'none' = chain structure only, no certificate layer to show
let certsMode = 'compact';
// true from the drawer opening until the engine's feed answers; the snapshot is dropped meanwhile
let loading = false;
let selectEntry = () => {};

// smr.js owns the report, so it resolves a committed entry to a command index
export function useEntrySelect(fn) { selectEntry = fn; }

const BW = 84, GAP = 22, X0 = 14, BY = 6, BH = 44, LVL = 64;
const SKYLINE_PEAK_HEIGHT = 7; // trees taller than 2^6 leaves → skyline

// Read side for the block drawer: the best server's forest, the committed
// entries it covers and how certificates are fed (compact / recovery / none).
export function forestView() {
  if (!snap) return null;
  const srv = best(snap);
  return { m: srv.m, peaks: srv.peaks, entries: snap.committed || null, mode: certsMode };
}
export function tallyFor(client, sn) {
  return tallies ? { round: tallies.round, tally: tallies.map[client]?.[sn] ?? null } : null;
}
export function cmdIndexFor(ent) {
  const i = cmdIndexMap().get(ent.client + ':' + ent.sn);
  return i === undefined ? -1 : i;
}

export const certsLoading = () => loading;
export function markCertsLoading() {
  resetCertsDrawer();
  loading = true;
}
export function endCertsLoading() { loading = false; }

export function resetCertsDrawer() {
  loading = false;
  snap = null;
  selPos = null;
  tallies = null;
  certsMode = 'compact';
  $('certVerStamp').textContent = '';
  cancelAnim();
  $('chainPanel').classList.add('hidden');
}

export function onVerifyTallies(json) {
  const v = JSON.parse(json);
  if (v.error) return;
  const map = {};
  for (const c of v.clients) {
    map[c.client] = {};
    for (const r of c.rows) map[c.client][r.sn] = r.tally;
  }
  tallies = { round: v.round, map };
  $('certVerStamp').textContent = 'verified @r' + v.round;
  if (snap) renderClients();
}

// `rq11` marks the recovery flavour: same JSON shape, but the certificates
// come from checkpoint-resident forests the paper never constructs, so the
// badge has to say so and an error freezes rather than wipes the panel.
export function onCertsSnapshot(certsJson, rq11 = false) {
  if (!certsJson) return;
  const next = JSON.parse(certsJson);
  if (next.error) {
    if (rq11) freezeChain(next.error);
    else resetCertsDrawer();
    return;
  }
  certsMode = rq11 ? 'recovery' : 'compact';
  show(next, mergesForBest(next));
}

// Recovery feed: the accumulated chain model smr.js maintains, reshaped into
// the one-server snapshot every painter already speaks.
export function onChainSnapshot(model) {
  if (!model) return;
  certsMode = 'none';
  show({
    servers: [{ server: 0, m: model.m, peaks: model.peaks }],
    merges_last_step: model.merges.map(e => ({ server: 0, ...e })),
    committed: model.entries,
    clients: [],
  }, model.merges);
}

// A violated safety oracle stops the engine exporting a chain at all, so no
// later render can overwrite this — the panel keeps its last good state and
// says why it stopped there.
export function freezeChain(message) {
  if (!snap) return;
  $('certRoots').className = 'badge badge-dead';
  $('certRoots').textContent = message;
}

function show(next, merges) {
  $('chainPanel').classList.remove('hidden');
  snap = next;
  if (merges.length > 0) playCascade(merges);
  else {
    cancelAnim();
    render(null, null);
  }
}

function best(s) {
  return s.servers.reduce((a, b) => (b.m > a.m ? b : a), s.servers[0]);
}
function mergesForBest(s) {
  const srv = best(s);
  return s.merges_last_step.filter(e => e.server === srv.server);
}
function cancelAnim() {
  anim.forEach(clearTimeout);
  anim = [];
}

// Pop at bit 0, then flash each carried level, 420 ms apart; a newer
// snapshot cancels the tail and renders final state (no backlog on autoplay).
// A recovery boundary commits a whole batch at once, so the carry chain can
// run to hundreds of merges — sample it down and shorten the step so the
// burst stays a burst instead of a minute-long slideshow. Compact commits one
// entry per round and never reaches the cap, so its cascade is unchanged.
const CASCADE_STEP_MS = 420, CASCADE_TOTAL_MS = 3000, CASCADE_MAX_FLASHES = 16;
function playCascade(merges) {
  cancelAnim();
  render(0, null, null);
  const many = merges.length > CASCADE_MAX_FLASHES;
  const shown = many
    ? Array.from({ length: CASCADE_MAX_FLASHES }, (_, i) =>
        merges[Math.round((i + 1) * (merges.length - 1) / CASCADE_MAX_FLASHES)])
    : merges;
  const step = many ? CASCADE_TOTAL_MS / CASCADE_MAX_FLASHES : CASCADE_STEP_MS;
  let t = step;
  for (const e of shown) {
    anim.push(setTimeout(() => render(null, e.height + 1, e), t));
    t += step;
  }
  anim.push(setTimeout(() => render(null, null, null), t));
}

function render(popBit, flashBit, flashEvent) {
  if (!snap) return;
  const srv = best(snap);
  const m = srv.m;

  $('certM').textContent = 'm = ' + m;
  $('certBin').textContent = m > 0 ? m.toString(2) : '0';
  $('certTrees').textContent = 'trees ' + srv.peaks.length;
  const hasCerts = certsMode !== 'none';
  if (hasCerts) {
    const ok = snap.roots_consistent;
    $('certRoots').className = 'badge ' + (ok ? 'badge-ok' : 'badge-dead');
    // The recovery badge must keep the claim honest: the paper asserts this
    // adaptation in one sentence (§6 p. 30) and proves Thm 6 for compact only.
    $('certRoots').textContent = !ok
      ? 'FORK'
      : certsMode === 'recovery'
        ? 'certificates — RQ11 construction (beyond paper)'
        : 'roots ✓';
    $('certSrv').textContent = 'server #' + srv.server;
  } else {
    // No roots-agree claim is available: Alg 6 servers keep no forest, so
    // this one is the lab's, over the one canonical committed sequence.
    $('certRoots').className = 'badge badge-run';
    $('certRoots').textContent = 'structure only — no certificates';
    $('certSrv').textContent = '';
  }
  $('chainTitle').textContent = hasCerts
    ? 'Committed sequence — §5 Merkle forest'
    : 'Committed sequence — hanging Merkle forest';
  $('certVerifyBtn').classList.toggle('hidden', !hasCerts);
  $('certClients').classList.toggle('hidden', !hasCerts);
  $('chainCertHint').classList.toggle('hidden', !hasCerts);
  $('chainRecHint').classList.toggle('hidden', hasCerts);

  const skyline = srv.peaks.some(p => p.height >= SKYLINE_PEAK_HEIGHT);
  // forest mode flashes the merged node itself; the panel-border glow only
  // remains for the skyline, where bars are the finest grain there is
  const flash = popBit !== null || flashBit !== null;
  $('chainPanel').classList.toggle('flash', flash && skyline);

  $('forestSvg').classList.toggle('hidden', skyline);
  $('certSky').classList.toggle('hidden', !skyline);
  $('certDetail').classList.toggle('hidden', !skyline);
  if (skyline) renderSkyline(srv, popBit, flashBit);
  else renderForest(srv, flashEvent ?? null);
  if (hasCerts) renderClients();
}

/* ---------- fused chain + hanging forest (small m) ---------- */

function svgEl(tag, attrs, text) {
  const e = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const k in attrs) e.setAttribute(k, attrs[k]);
  if (text != null) e.textContent = text;
  return e;
}

function renderForest(srv, flashEvent) {
  const svg = $('forestSvg');
  const scroller = $('forestScroll');
  // stay pinned to the newest block only if the user hasn't scrolled back
  const atTail = scroller.scrollLeft + scroller.clientWidth >= scroller.scrollWidth - (BW + GAP);
  svg.innerHTML = '';
  const m = srv.m;
  const slots = m;
  const bx = p => X0 + (p - 1) * (BW + GAP);
  const cx = p => bx(p) + BW / 2;
  const topY = BY + BH;

  // peak covering a position, path depth = peak height
  const peakOf = p => srv.peaks.find(pk => p > pk.start && p <= pk.start + 2 ** pk.height);
  const selPeak = selPos !== null ? peakOf(selPos) : null;
  // selected path: interval [lo,hi] at each depth from leaf up to the root
  const selIntervals = [];
  if (selPeak) {
    let lo = selPos, hi = selPos;
    for (let d = 0; d <= selPeak.height; d++) {
      selIntervals.push([lo, hi, d]);
      const span = 2 ** (d + 1);
      const rel = Math.floor((lo - selPeak.start - 1) / span);
      lo = selPeak.start + rel * span + 1;
      hi = Math.min(selPeak.start + (rel + 1) * span, selPeak.start + 2 ** selPeak.height);
    }
  }
  const onSelPath = (lo, hi, d) => selIntervals.some(([a, b, sd]) => sd === d && a === lo && b === hi);

  let maxDepthDrawn = 1;
  // hanging trees: level counts up from the leaf row (blocks) — leaf-pair
  // join = 1 … peak root = height
  for (const pk of srv.peaks) {
    const lo = pk.start + 1, hi = pk.start + 2 ** pk.height;
    if (pk.height === 0) {
      // single-leaf tree: its root IS the leaf — mark with a root chip
      svg.appendChild(svgEl('text', {
        x: cx(hi), y: topY + 16, 'text-anchor': 'middle', fill: '#6f7986',
        'font-size': 9, 'font-family': 'ui-monospace, Menlo, monospace',
      }, String(pk.root_hex).slice(0, 8)));
      continue;
    }
    drawRoot(pk, lo, hi);
  }
  function drawRoot(pk, lo, hi) {
    // depth here: 0 at leaf row … pk.height at root; drawSub expects the
    // interval + its depth measured from the root; re-map: walk recursively
    // from the full interval with depth = pk.height at the root.
    const walk = (l, h, level) => {
      // level = distance from leaves (leaf pair join = 1 … root = pk.height)
      const x = (cx(l) + cx(h)) / 2;
      const y = topY + level * LVL;
      const isSel = onSelPath(l, h, level);
      // the carry's freshly merged parent: interval [start+1, start+2^(h+1)]
      // one level above the merged children (MergeEvent.height = child level)
      const isFlash = flashEvent !== null
        && level === flashEvent.height + 1
        && l === flashEvent.start + 1
        && h === flashEvent.start + 2 ** (flashEvent.height + 1);
      maxDepthDrawn = Math.max(maxDepthDrawn, level);
      svg.appendChild(svgEl('circle', {
        cx: x, cy: y, r: 5.5,
        fill: isSel ? '#0f1216' : '#1c222a',
        stroke: isSel ? '#3dd6c4' : '#3a4450',
        'stroke-width': isSel ? 2 : 1.2,
        opacity: isSel ? 1 : Math.max(0.15, 0.85 - level * 0.18),
        ...(isFlash ? { class: 'nodeFlash' } : {}),
      }));
      if (level === pk.height) {
        svg.appendChild(svgEl('text', {
          x: x + 11, y: y + 4, fill: isSel ? '#3dd6c4' : '#6f7986',
          'font-size': 11, 'font-weight': 700,
          'font-family': 'ui-monospace, Menlo, monospace',
        }, String(pk.root_hex).slice(0, 8)));
      }
      // children live one level closer to the leaves
      if (level > 1) {
        const mid = l + (h - l + 1) / 2 - 1;
        for (const [cl, ch] of [[l, mid], [mid + 1, h]]) {
          const child = walk(cl, ch, level - 1);
          if (child) linkTo(x, y, child.x, child.y, isSel && child.sel, isFlash);
        }
      } else {
        // leaf edges: anchor at each block's bottom-center
        for (let p = l; p <= h; p++) {
          const sel = selPos === p && isSel;
          linkTo(x, y, cx(p), topY, sel, isFlash);
        }
      }
      return { x, y, sel: isSel };
    };
    walk(lo, hi, pk.height);
  }
  function linkTo(x1, y1, x2, y2, sel, flash) {
    svg.appendChild(svgEl('line', {
      x1: x2, y1: y2, x2: x1, y2: y1,
      stroke: sel ? '#3dd6c4' : '#3a4450',
      'stroke-width': sel ? 2 : 1.3,
      opacity: sel ? 1 : 0.55,
      ...(flash ? { class: 'edgeFlash' } : {}),
    }));
  }

  // chain row on top of everything: links, blocks, ghost slot
  for (let p = 1; p < m; p++) {
    svg.appendChild(svgEl('line', {
      x1: bx(p) + BW, y1: BY + BH / 2, x2: bx(p + 1), y2: BY + BH / 2,
      stroke: '#2a5f58', 'stroke-width': 2,
    }));
  }
  for (let p = 1; p <= m; p++) {
    const newest = p === m;
    const g = svgEl('g', { cursor: 'pointer' });
    if (newest || p === selPos) {
      g.appendChild(svgEl('rect', {
        x: bx(p) - 3, y: BY - 3, width: BW + 6, height: BH + 6, rx: 10,
        fill: 'none', stroke: p === selPos ? 'rgba(61,214,196,.6)' : 'rgba(61,214,196,.35)',
        'stroke-width': p === selPos ? 3 : 6,
      }));
    }
    g.appendChild(svgEl('rect', {
      x: bx(p), y: BY, width: BW, height: BH, rx: 8,
      fill: '#141a1e', stroke: newest ? '#3dd6c4' : '#2a5f58', 'stroke-width': newest ? 1.6 : 1,
    }));
    const ent = snap.committed ? snap.committed[p - 1] : null;
    g.appendChild(svgEl('text', {
      x: cx(p), y: ent ? BY + 20 : BY + 27, 'text-anchor': 'middle',
      fill: newest ? '#3dd6c4' : '#c9d2dd', 'font-size': 13, 'font-weight': 700,
      'font-family': 'ui-monospace, Menlo, monospace',
    }, '#' + p));
    if (ent) {
      const lab = entryLabel(ent);
      g.appendChild(svgEl('text', {
        x: cx(p), y: BY + 36, 'text-anchor': 'middle',
        fill: lab.color, 'font-size': 10, 'font-weight': 600,
        'font-family': 'ui-monospace, Menlo, monospace',
      }, lab.text));
      const tip = svgEl('title', {});
      tip.textContent = lab.title + ' — pos ' + p;
      g.appendChild(tip);
    }
    g.addEventListener('click', () => {
      selPos = selPos === p ? null : p;
      render(null, null);
      if (selPos !== null && ent) selectEntry(ent);
    });
    svg.appendChild(g);
  }
  // ghost slot: where the next commit lands
  const gp = m + 1;
  svg.appendChild(svgEl('line', {
    x1: bx(m) + BW, y1: BY + BH / 2, x2: bx(gp), y2: BY + BH / 2,
    stroke: '#2a5f58', 'stroke-width': 2, 'stroke-dasharray': '4 4', opacity: 0.5,
  }));
  svg.appendChild(svgEl('rect', {
    x: bx(gp), y: BY, width: BW, height: BH, rx: 8,
    fill: 'none', stroke: '#3a4450', 'stroke-dasharray': '5 5',
  }));
  svg.appendChild(svgEl('text', {
    x: cx(gp), y: BY + BH / 2 + 4, 'text-anchor': 'middle', fill: '#565f6b',
    'font-size': 12, 'font-family': 'ui-monospace, Menlo, monospace',
  }, '+'));

  // proof chip for the selected position (from its client cert row)
  const cert = selPos !== null ? certAt(selPos) : null;
  if (cert) {
    const chipX = Math.min(cx(selPos), (slots + 1) * (BW + GAP) - 70);
    const label = `c${cert.client} · sn ${cert.sn} · pos ${selPos}` +
      (cert.chain_len != null ? ` · chain ${cert.chain_len}` : '');
    const tw = label.length * 6.4 + 16;
    const chip = svgEl('g', {});
    chip.appendChild(svgEl('rect', {
      x: chipX - tw / 2, y: topY + 6, width: tw, height: 20, rx: 10,
      fill: '#12191c', stroke: '#2a5f58',
    }));
    chip.appendChild(svgEl('text', {
      x: chipX, y: topY + 20, 'text-anchor': 'middle', fill: '#3dd6c4',
      'font-size': 10, 'font-family': 'ui-monospace, Menlo, monospace',
    }, label));
    svg.appendChild(chip);
  }

  svg.setAttribute('width', X0 + (slots + 1) * (BW + GAP) + 120);
  svg.setAttribute('height', topY + maxDepthDrawn * LVL + 42);
  if (atTail) scroller.scrollLeft = scroller.scrollWidth;
}

// center a committed position's block in the scroll viewport
function scrollToPos(p) {
  const el = $('forestScroll');
  el.scrollLeft = Math.max(0, X0 + (p - 1) * (BW + GAP) + BW / 2 - el.clientWidth / 2);
}

// Harness positions are 0-based (forest length at append); the chain row and
// skyline ranges are 1-based — convert once here.
const blockPos = r => (r.pos == null ? null : r.pos + 1);

// (client, sn) → report-command index, for PALETTE colors matching the audit
// swatches and spread curves. Report order is (injection_round, client) with a
// stable sort, so a client's rank among its own commands IS its sn.
let idxCache = { cmds: null, map: null };
function cmdIndexMap() {
  const cmds = (S.smr.report && S.smr.report.commands) || [];
  if (idxCache.cmds !== cmds) {
    const map = new Map();
    const seen = new Map();
    cmds.forEach((c, i) => {
      const sn = (seen.get(c.client) || 0) + 1;
      seen.set(c.client, sn);
      map.set(c.client + ':' + sn, i);
    });
    idxCache = { cmds, map };
  }
  return idxCache.map;
}

// block label: who occupies the committed slot. The recovery feed carries op
// (the identity its audit trail and batch drawer use); compact carries sn.
export function entryLabel(ent) {
  if (ent.kind === 'nop') return { text: '·', color: '#565f6b', title: 'no-op filler' };
  const auto = ent.client >= AUTO_CLIENT_BASE;
  const own = ent.op != null ? '·op' + ent.op : '·' + ent.sn;
  const ownFull = ent.op != null ? ' · op ' + ent.op : ' · sn ' + ent.sn;
  const id = auto ? 'bg' + (ent.client - AUTO_CLIENT_BASE) : 'c' + ent.client + own;
  const full = auto ? 'bg #' + (ent.client - AUTO_CLIENT_BASE) : 'c' + ent.client + ownFull;
  if (ent.kind === 'null') return { text: '⊥', color: '#565f6b', title: '⊥ duplicate of ' + full };
  const i = cmdIndexMap().get(ent.client + ':' + ent.sn);
  const color = i === undefined ? '#c9d2dd' : PALETTE[i % PALETTE.length];
  return { text: id, color, title: full };
}

function certAt(pos) {
  for (const c of snap.clients || []) {
    for (const r of c.certs) if (blockPos(r) === pos) return { client: c.client, ...r };
  }
  return null;
}

/* ---------- skyline fallback (big m) ---------- */

let selBit = null;
function renderSkyline(srv, popBit, flashBit) {
  const peaks = srv.peaks;
  const m = srv.m;
  const byBit = {};
  peaks.forEach(p => { byBit[p.height] = p; });
  const maxBit = Math.max(5, m > 0 ? Math.floor(Math.log2(m)) : 0);
  const sky = $('certSky');
  sky.innerHTML = '';
  for (let bit = maxBit; bit >= 0; bit--) {
    const p = byBit[bit];
    const slot = document.createElement('div');
    slot.className = 'skySlot' + (p ? ' on' : '') + (p && selBit === bit ? ' sel' : '');
    if (p) {
      const bar = document.createElement('div');
      bar.className = 'skyBar' + (flashBit === bit ? ' flash' : '') + (popBit === bit ? ' pop' : '');
      bar.style.height = (8 + bit * 9) + 'px';
      slot.appendChild(bar);
    } else {
      const dot = document.createElement('div');
      dot.className = 'skyDot';
      slot.appendChild(dot);
    }
    const label = document.createElement('span');
    label.className = 'skyE';
    label.textContent = String(bit);
    slot.appendChild(label);
    slot.addEventListener('click', () => { selBit = bit; render(null, null); });
    sky.appendChild(slot);
  }
  const show = (selBit != null && byBit[selBit]) || peaks[peaks.length - 1];
  $('certDetail').innerHTML = show
    ? `<span>peak</span><b>2^${show.height} = ${(2 ** show.height).toLocaleString()} leaves · pos ${show.start + 1}–${show.start + 2 ** show.height}</b><span class="rt">root ${String(show.root_hex).slice(0, 8)}</span>`
    : '<span>no commits yet</span>';
}

/* ---------- client certificate tables ---------- */

function renderClients() {
  const box = $('certClients');
  const clients = snap.clients || [];
  if (clients.length === 0) {
    box.innerHTML = '<div class="emptyWrap"><div class="emptyTitle">No client commands yet</div><div class="emptyHint">Submit a command from the inject popover — the client registers on its first inject; repeat injects from the same client id build its sn sequence.</div></div>';
    return;
  }
  // Thm 6 tally cell: teal only when every covering server accepted; red only
  // on a real rejection (accepted < covered) — uncovered ≠ rejected under β>0.
  const tallyCell = t => {
    if (t == null) return '<span class="cVer vf-na">—</span>';
    const bad = t.accepted < t.covered;
    return `<span class="cVer ${bad ? 'vf-bad' : 'vf-ok'}">${t.accepted}/${t.covered}</span>`;
  };
  box.innerHTML = clients.map(c => {
    const rows = c.certs.map(r => {
      const bp = blockPos(r);
      const pos = bp == null ? '—' : String(bp);
      const chain = r.chain_len == null ? '—' : String(r.chain_len);
      const st = r.committed ? '<span class="cStatus st-com">committed</span>' : '<span class="cStatus st-pend">pending</span>';
      const sel = bp != null && bp === selPos ? ' sel' : '';
      const t = tallies ? tallies.map[c.client]?.[r.sn] : null;
      return `<div class="certRow${sel}" data-pos="${bp ?? ''}"><span class="cSn">${r.sn}</span><span class="cPos">${pos}</span><span class="cChain">${chain}</span>${st}${tallyCell(t)}</div>`;
    }).join('');
    // frozen §5 staleness demo row: green while the last-two window still
    // holds it, pink REJECTED once two further commits evicted it
    let staleRow = '';
    if (c.stale) {
      const rejected = c.stale.covered > 0 && c.stale.accepted === 0;
      const st = rejected ? '<span class="cStatus st-rej">REJECTED</span>' : '<span class="cStatus st-cap">captured</span>';
      staleRow = `<div class="certRow staleRow"><span class="cSn">${c.stale.sn}</span><span class="cPos">✱</span><span class="cChain">—</span>${st}${tallyCell(c.stale)}</div>`;
    }
    return `<div class="clCard"><div class="clHead"><span class="clName">c${c.client}</span><span class="clMeta">next sn ${c.next_sn}</span><button class="capBtn" data-client="${c.client}" title="Freeze this client's current bare-newest certificate (§5 staleness demo): the frozen row stays green until two further commits evict it, then flips REJECTED.">capture</button></div><div class="certHeadRow"><span>sn</span><span>pos</span><span>chain</span><span>status</span><span>verify</span></div>${rows}${staleRow}</div>`;
  }).join('');
  for (const row of box.querySelectorAll('.certRow[data-pos]')) {
    row.addEventListener('click', () => {
      const p = row.dataset.pos;
      if (p === '') return;
      selPos = selPos === +p ? null : +p;
      render(null, null);
      if (selPos !== null) scrollToPos(selPos);
    });
  }
  for (const btn of box.querySelectorAll('.capBtn')) {
    btn.addEventListener('click', () => post({ type: 'smr-capture-stale', client: +btn.dataset.client }));
  }
}
