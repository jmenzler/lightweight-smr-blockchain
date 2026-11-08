// Continuing a live run past its round target, shared by the single-value and SMR views. The lab
// keeps every round in page memory, so no target goes past ROUND_CAP.
export const ROUND_CAP = 5000;

// { rounds: X } adds X whole rounds (at least one), clamped to the cap; { open: true } runs until
// the user stops it, which is the cap. null when nothing is left under the cap.
export function continuePlan(current, mode, cap = ROUND_CAP) {
  if (current >= cap) return null;
  if (mode.open) return { target: cap, clamped: false, open: true };
  const want = current + Math.max(1, Math.round(mode.rounds));
  return { target: Math.min(cap, want), clamped: want > cap, open: false };
}

// A reached target ends the run; reaching the cap is its own end, which cannot continue.
export const targetEnd = (round, target, cap = ROUND_CAP) => ({ kind: target >= cap ? 'cap' : 'max', round, max: target });
