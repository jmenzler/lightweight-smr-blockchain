// Run from the repository root: node tools/lab/tests/continue_plan.mjs
// Continue on a run stopped at its round target: "+X rounds" clamped to the page-memory cap,
// or "until stopped", which runs to the cap; reaching the cap ends with its own status.
import assert from 'node:assert/strict';
import { ROUND_CAP, continuePlan, targetEnd } from '../continue.js';

assert.equal(ROUND_CAP, 5000);

assert.deepEqual(continuePlan(500, { rounds: 500 }), { target: 1000, clamped: false, open: false });
assert.deepEqual(continuePlan(4800, { rounds: 500 }), { target: 5000, clamped: true, open: false }, '+X stops at the cap and says so');
assert.deepEqual(continuePlan(500, { rounds: 0 }), { target: 501, clamped: false, open: false }, 'at least one round');
assert.deepEqual(continuePlan(500, { rounds: 12.6 }), { target: 513, clamped: false, open: false }, 'whole rounds');
assert.deepEqual(continuePlan(500, { open: true }), { target: 5000, clamped: false, open: true }, 'until stopped runs to the cap');
assert.equal(continuePlan(5000, { rounds: 10 }), null, 'nothing left under the cap');
assert.equal(continuePlan(5000, { open: true }), null);
assert.deepEqual(continuePlan(10, { rounds: 5 }, 12), { target: 12, clamped: true, open: false }, 'the cap is a parameter');

// a reached target ends as max, or as the lab limit when the target is the cap
assert.deepEqual(targetEnd(1000, 1000), { kind: 'max', round: 1000, max: 1000 });
assert.deepEqual(targetEnd(5000, 5000), { kind: 'cap', round: 5000, max: 5000 });

console.log('continue_plan: ok');
