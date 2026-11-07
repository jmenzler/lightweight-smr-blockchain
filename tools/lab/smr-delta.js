// Rebuilds the engine's full SMR report from per-step deltas (LiveSmr.report_delta_json).
// Never mutates `prev`: smr-derive.js and smr-certs.js memoize on the identity of the report,
// its commands array and its row arrays, so every merge hands back new ones.

export const EMPTY_REPORT = Object.freeze({ metrics: Object.freeze([]), commands: Object.freeze([]) });

function mergeCommands(old, delta) {
  const { inserted, changed, len } = delta;
  const out = new Array(len);
  let oldAt = 0;
  let insertAt = 0;
  for (let i = 0; i < len; i++) {
    if (insertAt < inserted.length && inserted[insertAt][0] === i) {
      out[i] = inserted[insertAt++][1];
    } else if (oldAt < old.length) {
      out[i] = old[oldAt++];
    } else {
      throw new Error(`report delta: no command for index ${i}`);
    }
  }
  if (oldAt !== old.length || insertAt !== inserted.length) {
    throw new Error(`report delta: ${old.length} commands and ${inserted.length} insertions do not make ${len}`);
  }
  for (const [index, command] of changed) {
    if (!(index >= 0 && index < len)) throw new Error(`report delta: changed index ${index} outside ${len} commands`);
    out[index] = command;
  }
  for (const [index, scalars, spreadFrom, spreadTail, attemptsFrom, attemptsTail] of delta.appended) {
    if (!(index >= 0 && index < len)) throw new Error(`report delta: appended index ${index} outside ${len} commands`);
    const was = out[index];
    if (was.spread.length !== spreadFrom) throw new Error(`report delta: spread of command ${index} has ${was.spread.length} points, the delta continues from ${spreadFrom}`);
    if (was.attempts.length !== attemptsFrom) throw new Error(`report delta: attempts of command ${index} number ${was.attempts.length}, the delta continues from ${attemptsFrom}`);
    out[index] = { ...scalars, spread: was.spread.concat(spreadTail), attempts: was.attempts.concat(attemptsTail) };
  }
  return out;
}

export function applyReportDelta(prev, delta) {
  if (delta.rounds_from !== prev.metrics.length) {
    throw new Error(`report delta: starts at round ${delta.rounds_from}, the page holds ${prev.metrics.length}`);
  }
  const prevRows = prev.recovery ? prev.recovery.rounds : [];
  if (delta.rec_rows_from !== prevRows.length) {
    throw new Error(`report delta: starts at recovery row ${delta.rec_rows_from}, the page holds ${prevRows.length}`);
  }
  const report = {
    ...delta.summary,
    metrics: prev.metrics.concat(delta.metrics),
    commands: mergeCommands(prev.commands, delta),
  };
  if (delta.summary.recovery) {
    report.recovery = { ...delta.summary.recovery, rounds: prevRows.concat(delta.rec_rows) };
  }
  return report;
}
