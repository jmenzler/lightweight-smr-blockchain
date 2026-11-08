// Series and events derived from one SmrReport. Pure: no DOM and no session state (state.js
// supplies a constant only), so the page, the viewport and the drawers all read the same numbers.
import { AUTO_CLIENT_BASE } from './state.js';

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

// recovery: the boundary every useful server executed it · compact: the first
// round every useful server executed it · extended: it has no commit, so the
// round it reached every log.
export function commitRound(c, proto) {
  const r = proto === 'recovery' ? c.executed_round : proto === 'compact' ? c.committed_round : c.all_logs_round;
  return r === undefined ? null : r;
}

// client:sn → report index; sn is a command's 1-based order among its client's commands.
export function snIndex(cmds) {
  const map = new Map();
  const seen = new Map();
  cmds.forEach((c, i) => {
    const sn = (seen.get(c.client) || 0) + 1;
    seen.set(c.client, sn);
    map.set(c.client + ':' + sn, i);
  });
  return map;
}

const AUTO_OP_BASE = 2 ** 40;

// The servers whose log holds `cmd` now, from the step status. Keyed by op: a pool client
// reused after its ack can have two commands spreading at once.
export function spreadingHolders(cmd, spreading) {
  const sp = spreading.find(s => s.op === cmd.op);
  return sp ? new Set(sp.holders) : cmd.all_logs_round !== null ? 'all' : new Set();
}

// `bg #j` is a traffic arrival's index (its op − AUTO_OP_BASE): one per command under both
// client models. Under unique it equals the client slot, so those labels are unchanged.
export const arrivalIndex = cmd => cmd.op - AUTO_OP_BASE;
export const clientName = client => (client >= AUTO_CLIENT_BASE ? `p${client - AUTO_CLIENT_BASE}` : `c${client}`);
export const cmdChip = cmd => (cmd.auto ? `bg #${arrivalIndex(cmd)}` : `c${cmd.client}·op${cmd.op}`);
export const cmdName = (cmd, pool) => (cmd.auto
  ? `bg #${arrivalIndex(cmd)}` + (pool ? ` · ${clientName(cmd.client)}` : '')
  : `c${cmd.client} · op ${cmd.op}`);

export const isPool = spec => spec?.client_model?.kind === 'pool';
export const clientModelSpec = (kind, clients) => (kind === 'pool'
  ? { kind: 'pool', clients: Math.max(1, Math.round(+clients || 1)) }
  : null);

// Rough mean commands in flight: mean arrivals per round × the rounds a command keeps its
// client (recovery: up to two windows to execute; compact: T to age). Measured peaks at the
// lab's n = 512, T = 48 defaults run 1.5–1.8× this.
export function poolInFlight(pmf, proto, T) {
  if (!pmf || !T) return 0;
  const total = pmf.reduce((a, w) => a + w, 0);
  const mean = pmf.reduce((a, w, i) => a + i * w, 0) / total;
  return mean * (proto === 'recovery' ? 2 * T : T);
}

// Attempt outcomes after which the target holds the command in its log with that round's stamp.
const ACCEPTED = { recovery: 'Delivered', compact: 'Amplified' };

// Latest accepted copy that could still delay the commit. A copy stamped from round r on is
// not yet aged at r, so compact counts copies before committed_round; a recovery boundary
// commits the P minted one window earlier, so copies from that minting boundary on are moot.
function latestStamp(c, proto, T) {
  const end = commitRound(c, proto);
  const cutoff = end === null ? Infinity : proto === 'recovery' ? end - T : end;
  let hi = c.delivered_round;
  for (const a of c.attempts || []) {
    if (a.outcome === ACCEPTED[proto] && a.round < cutoff && a.round > hi) hi = a.round;
  }
  return hi;
}

const marginClass = (margin, T) => (margin === null ? 'unknown' : margin < 0 ? 'aged' : 4 * margin < T ? 'tight' : 'ok');
const known = v => v !== null && v !== undefined;

let llMemo = { cmds: null, entries: null, T: null, proto: null, out: null };

// A command executes once every entry of the committed prefix through it is T rounds old
// (compact.rs aged_prefix_len), so deadline(pos) = max stamp over positions 1..pos, plus T.
// Stamps: a command's first is its delivered_round (lower bound), its latest accepted
// resend the upper; a nop is Nop(round) (DET-11), its op where the feed carries it. ⊥ and
// op-less nops have no known stamp and are skipped, never guessed.
// rows[pos − 1] = { pos, kind, cmd, stamp, stampHi, dlLo, dlHi, delivered, allLogs, fixed,
// end, margin, cls, skipped }; margin = dlLo − prefix_fixed_round.
export function lifelines(cmds, entries, T, proto) {
  if (!ACCEPTED[proto]) return null;
  const m = llMemo;
  if (m.cmds === cmds && m.entries === entries && m.T === T && m.proto === proto) return m.out;
  const index = snIndex(cmds);
  const skips = { nop: 0, null: 0, cmd: 0 };
  let lo = null, hi = null, skipped = 0;
  const rows = entries.map((ent, i) => {
    let stamp = null, stampHi = null, c = null, ci = -1;
    if (ent.kind === 'cmd') {
      const at = index.get(ent.client + ':' + ent.sn);
      ci = at === undefined ? -1 : at;
      c = ci < 0 ? null : cmds[ci];
      if (c && known(c.delivered_round)) {
        stamp = c.delivered_round;
        stampHi = latestStamp(c, proto, T);
      } else skips.cmd++;
    } else if (ent.kind === 'nop' && known(ent.op)) {
      stamp = stampHi = Number(ent.op);
    } else skips[ent.kind === 'nop' ? 'nop' : 'null']++;
    if (stamp === null) skipped++;
    else {
      lo = lo === null ? stamp : Math.max(lo, stamp);
      hi = hi === null ? stampHi : Math.max(hi, stampHi);
    }
    const dlLo = lo === null ? null : lo + T;
    const fixed = c && known(c.prefix_fixed_round) ? c.prefix_fixed_round : null;
    const margin = dlLo !== null && fixed !== null ? dlLo - fixed : null;
    const slack = proto === 'recovery' ? slackToCheckpoint(dlLo, fixed, T) : null;
    return {
      pos: i + 1, kind: ent.kind, cmd: ci, stamp, stampHi, dlLo, dlHi: hi === null ? null : hi + T,
      delivered: c ? c.delivered_round : null, allLogs: c ? c.all_logs_round : null,
      fixed, end: c ? commitRound(c, proto) : null,
      margin, cls: c ? marginClass(margin, T) : null,
      slack, slackCls: c && proto === 'recovery' ? marginClass(slack, T) : null, skipped,
    };
  });
  const out = { rows, skips };
  llMemo = { cmds, entries, T, proto, out };
  return out;
}

// Recovery only acts at window ends: the first boundary at or after the deadline is the one
// that can freeze the entry into P, so the slack runs to that boundary, not to the deadline.
export function slackToCheckpoint(dlLo, fixed, T) {
  if (dlLo === null || fixed === null) return null;
  return Math.ceil(dlLo / T) * T - fixed;
}

// The margin a lifeline row shows: the deadline margin; recovery adds its slack to the
// checkpoint as `alt`.
export function shownMargin(row, proto) {
  return { value: row.margin, cls: row.cls, alt: proto === 'recovery' ? row.slack : null };
}

export function blockMinMargin(rows, proto) {
  let best = null;
  for (const r of rows) {
    if (r.cmd < 0) continue;
    const { value } = shownMargin(r, proto);
    if (value !== null && (best === null || value < best.value)) best = { value, pos: r.pos };
  }
  return best;
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

/* ---------- backlog stages ---------- */

// Where a not-yet-committed command waits, one stage at a time. Position fixed (T_E) is
// recorded by live sessions only, so without it ordering and waiting merge into one stage.
// Extended has no commit: its backlog ends once the command is in every log.
export function stageKeys(proto, fixedLive) {
  if (proto === 'extended') return ['client', 'spreading'];
  const ordered = fixedLive ? ['client', 'ordering', 'waiting'] : ['client', 'waiting'];
  return proto === 'recovery' ? [...ordered, 'frozen'] : ordered;
}

const orInf = v => (v === null || v === undefined ? Infinity : v);

// Stage edges [injection, …, commit]; stage k holds bounds[k] ≤ r < bounds[k + 1], so the
// stages tile the backlog interval [injection, commit). Recovery executes at mT the P frozen at
// (m − 1)T, so freeze = executed − T, or `frozenFrom` for the newest P, which has not executed
// when the run ends. A later edge caps an earlier one: a position fixed only after the freeze
// moves the command straight into frozen (`clamped` says so).
export function stageBounds(c, proto, T, fixedLive, frozenFrom) {
  const end = orInf(commitRound(c, proto));
  const bounds = [c.injection_round, orInf(c.delivered_round)];
  if (proto !== 'extended' && fixedLive) bounds.push(orInf(c.prefix_fixed_round));
  if (proto === 'recovery') bounds.push(c.executed_round != null ? c.executed_round - T : orInf(frozenFrom));
  bounds.push(end);
  let clamped = false;
  for (let k = bounds.length - 2; k >= 1; k--) {
    if (bounds[k] > bounds[k + 1]) {
      if (bounds[k] !== Infinity && k >= 2) clamped = true;
      bounds[k] = bounds[k + 1];
    }
  }
  for (let k = 1; k < bounds.length; k++) bounds[k] = Math.max(bounds[k], bounds[0]);
  return { bounds, clamped };
}

const pointAt = (c, round) => {
  const pts = c.spread;
  if (!pts || !pts.length) return null;
  const guess = pts[round - pts[0].round];
  return guess && guess.round === round ? guess : pts.find(p => p.round === round) ?? null;
};

// Commands frozen into P at boundary B: P(B) covers the positions after what the pass executes,
// [executed before the pass + |P(B − T)|, … + |P(B)|), read in the round-B positions (before the
// pass; the seed entry sits at position 0 until a pass executes it, and |P(B − T)| counts it).
// |P(B)| lands on the row after B. `among` restricts the scan to some commands.
export function frozenAt(rep, B, T, among = null) {
  const rows = recRows(rep), ms = rep.metrics;
  if (!rows || !rows[B] || !ms[B - 1]) return null;
  const lo = ms[B - 1].max_executed_len + (B - T >= T ? rows[B - T].max_cp_p_len : 0);
  const hi = lo + rows[B].max_cp_p_len;
  const out = new Set();
  for (const i of among ?? rep.commands.keys()) {
    const p = pointAt(rep.commands[i], B);
    if (p && p.pos_min != null && p.pos_min >= lo && p.pos_max < hi) out.add(i);
  }
  return out;
}

// The newest boundary with a recorded |P|, and its commands that have not executed yet. Two
// distinct checkpoints at that window leave the membership unknown: members null, forkK says why.
export function lastFrozenP(rep, T) {
  const rows = recRows(rep);
  if (!rows || !T) return null;
  const B = Math.floor((rep.metrics.length - 1) / T) * T;
  if (B < T) return null;
  if (rows[B].cp_fork_k > 1) return { boundary: B, members: null, forkK: rows[B].cp_fork_k };
  const open = [];
  rep.commands.forEach((c, i) => { if (c.executed_round == null) open.push(i); });
  return { boundary: B, members: frozenAt(rep, B, T, open) };
}

let stageMemo = { rep: null, key: null, out: null };

// counts[key][r] = commands in that stage at the end of round r; the stack sums to the backlog.
export function stageSeries(rep, proto, T, fixedLive) {
  const key = `${proto}:${T}:${fixedLive}`;
  if (stageMemo.rep === rep && stageMemo.key === key) return stageMemo.out;
  const N = rep.metrics.length;
  const keys = stageKeys(proto, fixedLive);
  const diff = keys.map(() => new Int32Array(N + 2));
  const last = proto === 'recovery' ? lastFrozenP(rep, T) : null;
  let clamped = 0;
  rep.commands.forEach((c, i) => {
    const frozenFrom = last?.members?.has(i) ? last.boundary : null;
    const s = stageBounds(c, proto, T, fixedLive, frozenFrom);
    if (s.clamped) clamped++;
    keys.forEach((_, k) => {
      const a = s.bounds[k], b = Math.min(s.bounds[k + 1], N + 1);
      if (a > N || a >= b) return;
      diff[k][a]++;
      diff[k][b]--;
    });
  });
  const counts = {};
  keys.forEach((k, j) => {
    const arr = new Int32Array(N + 1);
    let run = 0;
    for (let r = 1; r <= N; r++) { run += diff[j][r]; arr[r] = run; }
    counts[k] = arr;
  });
  const pending = derive(rep, proto).pending;
  for (let r = 1; r <= N; r++) {
    const sum = keys.reduce((a, k) => a + counts[k][r], 0);
    if (sum !== pending[r]) throw new Error(`backlog stages sum to ${sum}, the backlog is ${pending[r]} at r${r}`);
  }
  const out = { keys, counts, clamped, lastP: last ? { boundary: last.boundary, size: last.members ? last.members.size : null, forkK: last.forkK ?? 1 } : null };
  stageMemo = { rep, key, out };
  return out;
}

/* ---------- safety checks ---------- */

// One chip per property the engine checks. An executed-sequence check is vacuous until
// something executed: it cannot fail yet. Absent report fields mean 0 / none.
export function safetyChecks(rep, proto, safetyRound) {
  if (proto === 'extended') return [];
  // the counters are serialized only when non-zero, so absence means 0, but only in a report that
  // carries safety_ok at all: an older format records nothing and must not read as passing
  const recorded = 'safety_ok' in rep;
  const z = v => v ?? 0;
  // a window-end row is read before its pass, so the commands' commit rounds decide
  const vacuous = rep.metrics.every(m => m.max_executed_len === 0) && rep.commands.every(c => commitRound(c, proto) === null);
  const state = ok => (!recorded ? 'unrecorded' : !ok ? 'fail' : vacuous ? 'vacuous' : 'ok');
  const exec = (key, name, tip, ok, at, evidence) =>
    ({ key, name, tip, evidence, state: state(ok), at: ok ? null : at ?? null });
  const out = [
    exec('fork', 'no fork', 'safety_ok: executed sequences never diverge.',
      rep.safety_ok, safetyRound ?? rep.terminal?.Halted?.round, 'two executed sequences diverged'),
    exec('monotone', 'monotone', `Def 1.5: no executed entry retracted · ${z(rep.retractions)} retractions.`,
      z(rep.retractions) === 0, rep.first_retraction_round, `${z(rep.retractions)} retractions`),
    exec('useful', 'useful in step', `Def 1.4 at every round end · ${z(rep.strong_safety_violation_rounds)} rounds out of step (a lag count).`,
      z(rep.strong_safety_violation_rounds) === 0, rep.strong_safety_first_violation,
      `${z(rep.strong_safety_violation_rounds)} rounds with useful servers on different executed sequences`),
  ];
  if (proto === 'recovery') {
    const rows = recRows(rep) ?? [];
    const ff = rows.findIndex(x => x.cp_fork_k > 1);
    const lineageOk = rep.recovery?.fork_ok;
    const recState = ok => (rep.recovery ? (ok ? 'ok' : 'fail') : 'unrecorded');
    out.push(
      exec('boundary', 'in step at window ends', `Def 1.4 at window ends (Lemma 6.7) · ${z(rep.strong_safety_boundary_violations)} rounds.`,
        z(rep.strong_safety_boundary_violations) === 0, rep.strong_safety_boundary_first_violation,
        `${z(rep.strong_safety_boundary_violations)} window ends with useful servers out of step`),
      { key: 'lineage', name: 'checkpoint lineage', state: recState(lineageOk), at: lineageOk || !rep.recovery ? null : ff >= 0 ? ff + 1 : null,
        tip: `Lemma 6.5/6.7 lineage (fork_ok)${ff >= 0 ? ` · ${rows[ff].cp_fork_k} checkpoints at window ${rows[ff].window ?? '?'}` : ''}.`,
        evidence: ff >= 0 ? `${rows[ff].cp_fork_k} distinct checkpoints in window ${rows[ff].window ?? '?'} (r${ff + 1})` : 'the lineage oracle failed' },
      { key: 'committedCp', name: 'committed checkpoints', state: recState(rep.recovery?.commit_fork_round == null), at: rep.recovery?.commit_fork_round ?? null,
        tip: 'Lemma 6.7: no two committed checkpoints at one window end.',
        evidence: `two distinct committed checkpoints at window end r${rep.recovery?.commit_fork_round}` },
    );
  }
  out.push(exec('once', 'executes once', 'exec_dup_round: no (client, sn) executed twice.', rep.exec_dup_round == null, rep.exec_dup_round,
    `a (client, sn) executed twice at r${rep.exec_dup_round}`));
  return out;
}

// The checks indicator: failures first, vacuous checks counted as not applicable, a report
// without the fields as not recorded; a check mark only when something passed.
export function checksLabel(chips) {
  const n = st => chips.filter(c => c.state === st).length;
  if (n('unrecorded') === chips.length) return `? ${chips.length} not recorded`;
  const rest = [n('vacuous') ? `${n('vacuous')} not applicable` : null, n('unrecorded') ? `${n('unrecorded')} not recorded` : null].filter(Boolean);
  if (n('fail')) return [`✕ ${n('fail')} failed`, ...rest].join(' · ');
  if (!n('ok')) return `○ ${rest.join(' · ')}`;
  return [`✓ ${n('ok')} passed`, ...rest].join(' · ');
}

/* ---------- ordering ---------- */

// Commit rank − submission rank among the given commands (in commit order); commands injected in
// the same round are tied, so a command anywhere inside its round's group counts 0.
export function displacements(rep, order) {
  const sorted = order.map(i => rep.commands[i].injection_round).sort((a, b) => a - b);
  const lo = new Map(), hi = new Map();
  sorted.forEach((v, k) => { if (!lo.has(v)) lo.set(v, k); hi.set(v, k); });
  return order.map((i, p) => {
    const v = rep.commands[i].injection_round;
    return p < lo.get(v) ? p - lo.get(v) : p > hi.get(v) ? p - hi.get(v) : 0;
  });
}

export const withinShare = (d, k) => (d.length ? d.filter(x => Math.abs(x) <= k).length / d.length : null);

// The committed sequence as report indices; no-ops, ⊥ and unknown entries are skipped.
export function committedOrder(rep, entries) {
  return committedEntries(rep, entries).map(x => x.cmd);
}

// The same with each command's chain position (1-based); `offset` = entries before the slice.
// A no-op or ⊥ entry takes a position but is no command.
export function committedEntries(rep, entries, offset = 0) {
  const index = snIndex(rep.commands);
  const out = [];
  entries.forEach((e, j) => {
    if (e.kind !== 'cmd') return;
    const i = index.get(e.client + ':' + e.sn);
    if (i !== undefined) out.push({ cmd: i, pos: offset + j + 1 });
  });
  return out;
}

// Per client, commands commit in issue order (report order = sn order): a later command committed
// while an earlier one is not, or before it, is a violation. `order` must be the whole committed
// sequence; `scope` (a set of commands) only restricts which pairs are reported, by their later
// command. 'na' when no client issued two.
export function fifoCheck(rep, order, scope = null) {
  const pos = new Map(order.map((i, p) => [i, p]));
  const byClient = new Map();
  rep.commands.forEach((c, i) => { if (!byClient.has(c.client)) byClient.set(c.client, []); byClient.get(c.client).push(i); });
  let multi = 0, pairs = 0, violations = 0;
  for (const idx of byClient.values()) {
    if (idx.length > 1) multi++;
    for (let k = 1; k < idx.length; k++) {
      const a = pos.get(idx[k - 1]), b = pos.get(idx[k]);
      if (b === undefined || (scope && !scope.has(idx[k]))) continue;
      pairs++;
      if (a === undefined || a > b) violations++;
    }
  }
  return { clients: byClient.size, multi, pairs, violations, status: !pairs ? 'na' : violations ? 'fail' : 'ok' };
}

// The summary's "in view" scope: commands delivered inside the visible rounds.
export function inViewSet(rep, d0, d1) {
  const out = new Set();
  rep.commands.forEach((c, i) => { if (c.delivered_round != null && c.delivered_round >= d0 && c.delivered_round <= d1) out.add(i); });
  return out;
}

// A spread curve runs until one round after the command is in every log (holders are read at the
// round start, so that round shows the full share); past it a checkpoint adoption can dip the
// share for a round. A command not in every log yet runs to its peak.
export function spreadCurve(c) {
  const pts = c.spread || [];
  if (c.all_logs_round != null) return pts.filter(p => p.round <= c.all_logs_round + 1);
  let peak = 0;
  pts.forEach((p, j) => { if (p.useful_holders > pts[peak].useful_holders) peak = j; });
  return pts.slice(0, peak + 1);
}

// Median and p90 time spent in each backlog stage, over committed commands (optionally a subset).
export function stageLatencies(rep, proto, T, fixedLive, among) {
  const keys = stageKeys(proto, fixedLive);
  const per = keys.map(() => []);
  rep.commands.forEach((c, i) => {
    if (among && !among.has(i)) return;
    const { bounds } = stageBounds(c, proto, T, fixedLive, null);
    if (bounds[bounds.length - 1] === Infinity) return;
    keys.forEach((_, k) => per[k].push(bounds[k + 1] - bounds[k]));
  });
  return keys.map((key, k) => {
    const v = per[k].sort((a, b) => a - b);
    return { key, n: v.length, median: v.length ? percentile(v, 0.5) : null, p90: v.length ? percentile(v, 0.9) : null, byDefinition: key === 'frozen' };
  });
}

/* ---------- selection ---------- */

// Two levels wherever commands are marked: the selected block's commands are tinted, the one
// selected command is white on top of them.
export function cmdHighlight(i, sel, blockCmds) {
  if (sel === i) return 'command';
  return blockCmds && blockCmds.has(i) ? 'block' : 'none';
}

// { block: m } toggles a block (null closes it); { cmd: i } toggles a command. Leaving a block
// drops the command picked inside it.
export function nextSelection(state, action) {
  if ('block' in action) {
    if (action.block === null || action.block === state.blockSel) return { blockSel: null, sel: null };
    return { blockSel: action.block, sel: null };
  }
  return { blockSel: state.blockSel, sel: state.sel === action.cmd ? null : action.cmd };
}

// A block's life on the round axis, so the lanes can link its tile to where its commands were:
// first delivery, the spread (median delivery → median all-logs), median position fixed,
// the freeze (recovery: the tile's left edge) and the execution (the tile's round).
export function blockLife(rep, block, proto, T) {
  if (!block.cmds.length) return null;
  const cs = block.cmds.map(i => rep.commands[i]);
  const med = key => {
    const v = cs.map(c => c[key]).filter(x => x != null).sort((a, b) => a - b);
    return v.length ? v[Math.floor(v.length / 2)] : null;
  };
  const delivered = cs.map(c => c.delivered_round).filter(x => x != null);
  return {
    from: delivered.length ? Math.min(...delivered) : Math.min(...cs.map(c => c.injection_round)),
    spread: [med('delivered_round'), med('all_logs_round')],
    fixed: med('prefix_fixed_round'),
    frozen: proto === 'recovery' ? block.round - T : null,
    end: block.round,
  };
}

// Sidebar tab: Summary while nothing is selected, Audit for every new selection; a tab the user
// picks holds until the next selection.
export function sideTabAfter(state, event) {
  if ('pick' in event) return { tab: event.pick, explicit: true };
  if (event.select !== null) return { tab: 'audit', explicit: false };
  return state.explicit ? state : { tab: 'summary', explicit: false };
}

/* ---------- the block drawer's entry list ---------- */

// Submit-to-commit latency above which a committed command counts as slow: the run's p90.
export function slowAbove(rep, proto) {
  const v = [];
  for (const c of rep.commands) {
    const end = commitRound(c, proto);
    if (end !== null) v.push(end - c.injection_round);
  }
  v.sort((a, b) => a - b);
  return v.length ? percentile(v, 0.9) : Infinity;
}

// A Verify result indexed by client and sn: { round, rows: Map('client:sn' → tally) }.
export function verifyIndex(v) {
  const rows = new Map();
  for (const c of v.clients) for (const r of c.rows) rows.set(c.client + ':' + r.sn, r.tally);
  return { round: v.round, rows };
}

// What the newest Verify says about one command: null before any Verify; covered false when it
// has no row for it (a client it does not track, or committed after the verify round); else its
// tally, null when no certificate was constructible.
export function certRecord(index, client, sn) {
  if (!index) return null;
  const key = client + ':' + sn;
  if (!index.rows.has(key)) return { round: index.round, covered: false, tally: null };
  return { round: index.round, covered: true, tally: index.rows.get(key) };
}

// One mark per entry, by the state that matters most: no certificate (verified, none
// constructible) > slow > a command you injected > plain; no-ops and ⊥ are filler.
// `cert` is the command's certRecord.
export function entryMark(ent, cmd, proto, slowLimit, cert) {
  if (ent.kind !== 'cmd') return 'filler';
  if (cert?.covered && cert.tally === null) return 'nocert';
  const end = cmd ? commitRound(cmd, proto) : null;
  if (end !== null && end - cmd.injection_round > slowLimit) return 'slow';
  return cmd && !cmd.auto ? 'mine' : 'plain';
}

// The list's picked position after a render: a command selected elsewhere in this block wins; a
// command row loses its pick when the global selection is elsewhere or empty; a no-op or ⊥ pick stays.
export function syncedEntry(rows, sel, entry) {
  if (sel !== null) {
    const r = rows.find(x => x.cmd === sel);
    if (r) return r.pos;
  }
  const cur = rows.find(x => x.pos === entry);
  return cur && cur.cmd >= 0 ? null : entry;
}

// Picking a row (toggle: a click; else a key): the new pick and the global selection to set,
// undefined when it stays. A filler pick clears the global selection.
export function pickedEntry(rows, sel, entry, pos, toggle) {
  const r = rows.find(x => x.pos === pos);
  const again = toggle && entry === pos;
  const next = again ? null : pos;
  if (r && r.cmd >= 0) return { entry: next, select: again ? null : r.cmd };
  return { entry: next, select: sel !== null ? null : undefined };
}

// The rows a virtual list builds: those in view plus `pad` on each side, as [lo, hi).
export function listWindow(scrollTop, viewH, rowH, n, pad) {
  const lo = Math.max(0, Math.floor(scrollTop / rowH) - pad);
  const hi = Math.min(n, Math.ceil((scrollTop + viewH) / rowH) + pad);
  return [lo, hi];
}

/* ---------- rollbacks, stale sequences ---------- */

// A window end's pass lands on the row after it: rows[B] (round B + 1) carries boundary B.
export function rollbackRounds(rows) {
  const out = [];
  rows.forEach((row, i) => { if (row.rollbacks > 0) out.push({ round: i, count: row.rollbacks, depth: row.rollback_depth }); });
  return out;
}

// Compact reads its committed sequence from the certificate snapshot, which refreshes only while
// the block drawer is open; recovery's chain delta arrives every round.
export function staleNote(proto, seqRound, N) {
  if (seqRound == null || seqRound >= N) return null;
  return `committed sequence as of r${seqRound} · ` + (proto === 'compact' ? 'open a block to refresh' : 'the chain feed stopped');
}

/* ---------- memory ---------- */

export function formatBytes(b) {
  if (b === null || b === undefined) return '–';
  if (b < 1024) return `${b} B`;
  if (b < 1024 ** 2) return `${Math.round(b / 1024)} KiB`;
  if (b < 1024 ** 3) return `${(b / 1024 ** 2).toFixed(1)} MiB`;
  return `${(b / 1024 ** 3).toFixed(1)} GiB`;
}

// wasm32 linear memory tops out at 4 GiB and browsers often refuse earlier: warn from 1.5 GiB.
export function memoryLevel(linear) {
  if (!linear) return 'ok';
  return linear >= 3 * 1024 ** 3 ? 'high' : linear >= 1.5 * 1024 ** 3 ? 'warn' : 'ok';
}

/* ---------- servers lane ---------- */

// n/2 is Lemma 2.2's precondition on useful servers; ⌈3n/4⌉ its target, and the sim's
// recovered-round floor on non-⊥ logs. The axis crops to n/2–n while every series stays there.
export function serverAxis(rep, n) {
  const half = n / 2, q3 = Math.ceil((3 * n) / 4);
  const low = rep.metrics.reduce((a, m) => Math.min(a, m.nonbot_logs, m.useful, n - m.blocked), Infinity);
  const cropped = low >= half;
  return { lo: cropped ? half : 0, cropped, half, q3 };
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

// Block tiles. Recovery: one per window boundary (nothing commits
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

// Labelled events the lab already derives; nothing here is a new threshold. Rollbacks,
// r* and the safety checks have their own lane, marker and chips.
export function timelineEvents(rep, ctx) {
  const out = [];
  const dead = rep.metrics.findIndex(m => m.nonbot_logs === 0);
  if (dead >= 0) out.push({ round: dead + 1, kind: 'collapse', label: 'collapse', tip: `Every log is ⊥ from round ${dead + 1}; no server can answer a log request.` });
  const release = recRows(rep) ? releaseRound(rep, ctx.n) : null;
  if (release !== null) out.push({ round: release, kind: 'release', label: 'release', tip: `Blocking fell back below the surge level at round ${release}.` });
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
    case 'cap': return `stopped at the ${end.max}-round lab limit`;
  }
  throw new Error('unknown end kind ' + end.kind);
}
export const END_LABEL = { max: 'finished', dead: 'all ⊥', halted: 'halted', failed: 'FAILED', ended: 'ended', aborted: 'aborted', cap: 'lab limit' };

// A scheduled program is piecewise [from, to) round ranges, each a blocked
// percentage; outside every segment nothing is blocked. null = no program.
export function programBeta(program, round) {
  if (!program) return null;
  const seg = program.find(s => round >= s.from && round < s.to);
  return seg ? seg.beta : 0;
}
