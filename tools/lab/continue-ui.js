// The Continue ▾ dropdown of a live run stopped at its round target: "+X rounds" (X remembered on
// the session context, default the run's original max rounds) or "until stopped".
import { $ } from './state.js';
import { ROUND_CAP, continuePlan } from './continue.js';

export function bindContinueMenu(prefix, { ctx, current, go }) {
  const by = () => ctx().continueBy ?? ctx().spec.max_rounds;
  const refresh = () => {
    if (!ctx()) return;
    const plan = continuePlan(current(), { rounds: by() });
    if (document.activeElement !== $(prefix + 'ContBy')) $(prefix + 'ContBy').value = by();
    $(prefix + 'ContAdd').textContent = `+${by()} rounds`;
    $(prefix + 'ContAdd').dataset.tip = plan ? `to round ${plan.target}` + (plan.clamped ? ` (clamped to the ${ROUND_CAP}-round lab limit)` : '') : '';
  };
  $(prefix + 'ContBy').addEventListener('input', () => {
    const v = Math.round(+$(prefix + 'ContBy').value);
    if (v >= 1) { ctx().continueBy = v; refresh(); }
  });
  $(prefix + 'ContAdd').addEventListener('click', () => go({ rounds: by() }));
  $(prefix + 'ContOpen').addEventListener('click', () => go({ open: true }));
  return refresh;
}

export const continueStatus = plan => (plan.open
  ? `continuing until stopped (lab limit ${ROUND_CAP} rounds)`
  : `continuing to round ${plan.target}` + (plan.clamped ? ` (clamped to the ${ROUND_CAP}-round lab limit)` : ''));
