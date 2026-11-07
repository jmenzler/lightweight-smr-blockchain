// Series and events derived from one SmrReport. Pure: no DOM, no state module,
// so the page, the viewport and the drawers all read the same numbers.

export const recRows = rep => (rep.recovery ? rep.recovery.rounds : null);

// The same scan the engine's SmrReport::recovered_round performs, twinned in
// crates/sim-wasm/tests/smr_recovery_live.rs — any change to this predicate
// must change that test too.
export function rStarScan(rep, n, T) {
  if (rep.terminal?.Failed) return null;
  const rows = recRows(rep);
  if (!rows) return null;
  const ms = rep.metrics;
  const floor = Math.ceil(0.75 * n);
  const bs = [];
  for (let r = T; r <= ms.length; r += T) bs.push(r);
  const clean = b => rows[b - 1].reset === 0 && ms[b - 1].nonbot_logs >= floor;
  for (const rStar of bs) {
    const m = ms[rStar - 1];
    // A wasm build without the field reports all-server lengths only.
    const u = 'useful_executed' in m ? m.useful_executed : [m.min_executed_len, m.max_executed_len];
    if (!u || u[0] !== u[1]) continue;
    if (bs.every(b => b < rStar || clean(b))) return rStar;
  }
  return null;
}

// A surge round leaves fewer than n/4 servers unblocked; release = the first
// round after one with n/4 or more unblocked again. Counted on blocked servers
// because a blackout leaves every log ⊥, so the useful count stays 0 until the
// rollback and cannot mark the end of the blocking. Read off the recorded
// blocked counts, so it is whatever actually happened — slider, preset
// program or spec schedule alike.
export function releaseRound(rep, n) {
  const ms = rep.metrics;
  let surged = false;
  for (let i = 0; i < ms.length; i++) {
    if (4 * (n - ms[i].blocked) < n) { surged = true; continue; }
    if (surged) return i + 1;
  }
  return null;
}

// Latched fields land on the row after a boundary and are 0 elsewhere; a
// reader should keep showing the newest one, not blank between boundaries.
export function lastLatched(rows, key) {
  for (let i = rows.length - 1; i >= 0; i--) if (rows[i][key] !== 0) return rows[i][key];
  return 0;
}

// recovery: the boundary every useful server executed it · compact: the first
// round every useful server executed it · extended: it has no commit, so the
// round it reached every log.
export function commitRound(c, proto) {
  const r = proto === 'recovery' ? c.executed_round : proto === 'compact' ? c.committed_round : c.all_logs_round;
  return r === undefined ? null : r;
}

let memo = { rep: null, proto: null, data: null };

// pending[r] = submitted by round r and not yet committed; oldest[r] = rounds
// the oldest of those has waited since its injection (client wait time, which
// a rollback does not reset); committed[r] = commit landmarks reached by r.
export function derive(rep, proto) {
  if (memo.rep === rep && memo.proto === proto) return memo.data;
  const rounds = rep.metrics.length;
  const pending = new Array(rounds + 2).fill(0);
  const committed = new Array(rounds + 2).fill(0);
  const cmds = rep.commands.map(c => ({ inj: c.injection_round, done: commitRound(c, proto) }))
    .sort((a, b) => a.inj - b.inj);
  const delta = new Array(rounds + 3).fill(0);
  const doneAt = new Array(rounds + 3).fill(0);
  for (const c of cmds) {
    if (c.inj > rounds) continue;
    delta[c.inj] += 1;
    if (c.done !== null && c.done <= rounds) { delta[c.done] -= 1; doneAt[c.done] += 1; }
  }
  let live = 0, sum = 0;
  for (let r = 1; r <= rounds; r++) {
    live += delta[r];
    sum += doneAt[r];
    pending[r] = live;
    committed[r] = sum;
  }
  // The oldest pending command never moves past a still-pending older one, so
  // one pointer through the injection-ordered list finds it each round.
  const oldest = new Array(rounds + 2).fill(0);
  let lo = 0;
  for (let r = 1; r <= rounds; r++) {
    while (lo < cmds.length && cmds[lo].done !== null && cmds[lo].done <= r) lo++;
    oldest[r] = lo < cmds.length && cmds[lo].inj <= r ? r - cmds[lo].inj : 0;
  }
  const data = { pending, oldest, committed };
  memo = { rep, proto, data };
  return data;
}

function percentile(sorted, p) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(p * sorted.length) - 1)];
}

// Rounds a command needs to reach every non-⊥ log: all_logs_round − delivered
// (the T_B landmark). A command that has not reached every server yet is
// counted apart, so a surge cannot make the median look faster than it was.
export function spreadStats(cmds, n) {
  const times = [];
  let spreading = 0;
  for (const c of cmds) {
    if (c.delivered_round === null) continue;
    if (c.all_logs_round === null) spreading++;
    else times.push(c.all_logs_round - c.delivered_round);
  }
  times.sort((a, b) => a - b);
  const log2n = n > 1 ? Math.log2(n) : null;
  const median = times.length ? percentile(times, 0.5) : null;
  const p90 = times.length ? percentile(times, 0.9) : null;
  return {
    measured: times.length, spreading, median, p90, log2n,
    multiple: median !== null && log2n ? median / log2n : null,
  };
}

export function neverCommitted(rep, proto) {
  const open = rep.commands.filter(c => commitRound(c, proto) === null);
  const dead = open.filter(c => c.status === 'Dead').length;
  // Extended's client stops after its first delivery, and only an amplifying
  // receiver puts a command into any log: delivered with no amplify receiver
  // means it never entered a log. Compact and recovery clients resend, so the
  // case cannot occur there.
  const stranded = proto === 'extended'
    ? open.filter(c => c.status !== 'Dead' && c.delivered_round !== null && c.amp_receivers.length === 0).length : 0;
  return { total: open.length, dead, stranded, inFlight: open.length - dead - stranded };
}

const stats = xs => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length ? { avg: s.reduce((a, b) => a + b, 0) / s.length, median: percentile(s, 0.5), max: s[s.length - 1], n: s.length } : null;
};

// Chain lane blocks. Recovery: one per window boundary (nothing commits
// between boundaries, so a batch is the jump in executed length across
// consecutive post-boundary rows). Compact: one per window of T rounds,
// the window being a display unit only — compact commits continuously.
export function chainBlocks(rep, proto, T) {
  const ms = rep.metrics;
  if (!T || (proto !== 'recovery' && proto !== 'compact')) return [];
  const at = r => (r <= ms.length ? ms[r - 1].max_executed_len : null);
  const blocks = [];
  if (proto === 'recovery') {
    const rows = recRows(rep);
    if (!rows) return [];
    let rolledBack = false;
    for (let m = 1; (m - 1) * T + 1 <= ms.length; m++) {
      const here = at(m * T + 1);
      if (here === null) break;
      const from = at((m - 1) * T + 1);
      const size = Math.max(0, here - from);
      const drain = rolledBack && size > 0;
      if (drain) rolledBack = false;
      const rb = rows[m * T] ? rows[m * T].rollbacks : 0;
      const depth = rows[m * T] ? rows[m * T].rollback_depth : 0;
      if (rb > 0) rolledBack = true;
      const cmds = [];
      rep.commands.forEach((c, i) => { if (c.executed_round === m * T) cmds.push(i); });
      blocks.push({
        m, round: m * T, label: `b${m * T}`, from: from, size, drain, rb, depth, cmds,
        state: size === 0 ? (rb > 0 ? 'rollback' : 'empty') : drain ? 'drain' : 'committed',
        latency: stats(cmds.map(i => rep.commands[i].executed_round - rep.commands[i].injection_round)),
      });
    }
    return blocks;
  }
  const start = r => (r <= 0 ? 0 : ms[r - 1].max_executed_len);
  for (let m = 1; m * T <= ms.length; m++) {
    const lo = (m - 1) * T, hi = m * T;
    const cmds = [];
    rep.commands.forEach((c, i) => {
      if (c.committed_round != null && c.committed_round > lo && c.committed_round <= hi) cmds.push(i);
    });
    const size = Math.max(0, start(hi) - start(lo));
    // count and latency come from the same set: client commands whose
    // committed_round falls in the window; size is the executed-entry growth
    // that positions the block in the forest
    blocks.push({
      m, round: hi, label: `r${lo + 1}–${hi}`, from: start(lo), size, count: cmds.length, drain: false, rb: 0, depth: 0, cmds,
      state: cmds.length === 0 ? 'empty' : 'committed',
      latency: stats(cmds.map(i => rep.commands[i].committed_round - rep.commands[i].injection_round)),
    });
  }
  return blocks;
}

// Labelled events the lab already derives; nothing here is a new threshold.
// `safetyRound` is the round the live loop first saw safety_ok false, or null
// when no round was observed (a report loaded whole carries none).
export function timelineEvents(rep, ctx, safetyRound) {
  const out = [];
  const ms = rep.metrics;
  const rows = recRows(rep);
  const T = ctx.tw;
  if (rows && T) {
    for (let b = T; b < rows.length; b += T) {
      const row = rows[b];
      if (row.rollbacks > 0) {
        out.push({ round: b, kind: 'rollback', label: `⟲ ${row.rollbacks}`,
          tip: `Boundary ${b}: ${row.rollbacks} server${row.rollbacks === 1 ? '' : 's'} rolled back to their last checkpoint (up to ${row.rollback_depth} window${row.rollback_depth === 1 ? '' : 's'} back). Discarded entries are re-sent by their clients.` });
      }
    }
    for (let i = 1; i < rows.length; i++) {
      if (rows[i].max_checkpoint_window > rows[i - 1].max_checkpoint_window) {
        out.push({ round: i + 1, kind: 'checkpoint', label: '', tip: `Checkpoint window ${rows[i].max_checkpoint_window} reached at round ${i + 1}.` });
      }
    }
  }
  const dead = ms.findIndex(m => m.nonbot_logs === 0);
  if (dead >= 0) out.push({ round: dead + 1, kind: 'collapse', label: 'collapse', tip: `Every log is ⊥ from round ${dead + 1}; no server can answer a log request.` });
  const n = ctx.n;
  const release = rows ? releaseRound(rep, n) : null;
  if (release !== null) out.push({ round: release, kind: 'release', label: 'release', tip: `Blocking fell back below the surge level at round ${release}.` });
  const rStar = release !== null && T ? rStarScan(rep, n, T) : null;
  if (rStar !== null) out.push({ round: rStar, kind: 'confirmed', label: 'recovered', tip: lawText(release, rStar, T) });
  if (safetyRound !== null && safetyRound !== undefined) out.push({ round: safetyRound, kind: 'safety', label: 'SAFETY', tip: 'The engine reports a safety violation from this round.' });
  if (rep.exec_dup_round != null) out.push({ round: rep.exec_dup_round, kind: 'safety', label: 'double exec', tip: `First round an executed command repeated: ${rep.exec_dup_round}.` });
  return out.sort((a, b) => a.round - b.round);
}

export function lawText(release, rStar, T) {
  const phi = release % T;
  const predicted = 2 * T + ((T - phi) % T);
  const actual = rStar - release;
  return `Recovery took ${actual} rounds (release r${release}, recovered r${rStar}). R = 2T + (T − φ) mod T = 2·${T} + (${T} − ${phi}) mod ${T} = ${predicted}` +
    (actual === predicted ? ' ✓' : `; measured ${actual}. The law describes a full blackout released inside a window; this run skipped the re-arm window.`);
}

// Collapsed (all logs ⊥), recovering (release → r*), recovered (from r*).
export function phaseBands(rep, ctx) {
  const ms = rep.metrics;
  const bands = [];
  let start = null;
  ms.forEach((m, i) => {
    if (m.nonbot_logs === 0) { if (start === null) start = i + 1; }
    else if (start !== null) { bands.push({ kind: 'collapsed', from: start, to: i }); start = null; }
  });
  if (start !== null) bands.push({ kind: 'collapsed', from: start, to: ms.length });
  if (ctx.proto === 'recovery' && ctx.tw) {
    const release = releaseRound(rep, ctx.n);
    if (release !== null) {
      const rStar = rStarScan(rep, ctx.n, ctx.tw);
      if (rStar === null) bands.push({ kind: 'recovering', from: release, to: ms.length });
      else {
        bands.push({ kind: 'recovering', from: release, to: rStar });
        bands.push({ kind: 'recovered', from: rStar, to: ms.length });
      }
    }
  }
  return bands;
}

export function terminalText(rep) {
  const t = rep.terminal;
  if (t?.Failed) return `FAILED attempted r${t.Failed.attempted_round}, completed r${t.Failed.completed_round}, observed r${t.Failed.observed_round}`;
  if (t?.Dead) return `DEAD r${t.Dead.round}`;
  if (t?.Halted) return `HALTED (${t.Halted.kind}) r${t.Halted.round}`;
  return null;
}

// Why a stopped SMR run cannot step any more: { kind, round, max, detail }.
// A live session builds the same terminal shape from its step status.
export function smrEnd(terminal, max) {
  if (terminal?.Failed) return { kind: 'failed', round: terminal.Failed.attempted_round, max, detail: terminal.Failed };
  if (terminal?.Dead) return { kind: 'dead', round: terminal.Dead.round, max };
  if (terminal?.Halted) return { kind: 'halted', round: terminal.Halted.round, max, detail: terminal.Halted.kind };
  return { kind: 'max', round: terminal?.Ran?.rounds ?? max, max };
}
export function endText(end) {
  const at = `${end.round}/${end.max}`;
  switch (end.kind) {
    case 'max': return `finished · ${at} rounds`;
    case 'dead': return `⚠ finished · every server ⊥ at round ${at} — nothing can recover`;
    case 'halted': return `⚠ halted (${end.detail}) at round ${at}`;
    case 'failed': return `⚠ FAILED: attempted ${end.detail.attempted_round} · completed ${end.detail.completed_round} · observed ${end.detail.observed_round}`;
    case 'ended': return `ended · stopped at round ${at} by End`;
    case 'aborted': return `⚠ aborted at round ${at} — the engine refused to continue`;
  }
  throw new Error('unknown end kind ' + end.kind);
}
export const END_LABEL = { max: 'finished', dead: 'all ⊥', halted: 'halted', failed: 'FAILED', ended: 'ended', aborted: 'aborted' };

// A scheduled program is piecewise [from, to) round ranges, each a blocked
// percentage; outside every segment nothing is blocked. null = no program.
export function programBeta(program, round) {
  if (!program) return null;
  const seg = program.find(s => round >= s.from && round < s.to);
  return seg ? seg.beta : 0;
}
