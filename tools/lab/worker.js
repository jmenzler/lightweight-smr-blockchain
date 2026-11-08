// the ?v= cache-buster from index.html rides on the worker URL; forward it so
// the wasm glue + binary are fetched as fresh as the worker itself
const v = self.location.search || '';
importScripts('./sim_wasm.js' + v);

let ready = fetch('./sim_wasm_bg.wasm' + v)
  .then(r => {
    if (!r.ok) throw new Error('wasm fetch ' + r.status);
    return r.arrayBuffer();
  })
  .then(buf => {
    wasm_bindgen.initSync({ module: buf });
    postMessage({ type: 'ready' });
  })
  .catch(e => postMessage({ type: 'initfail', error: String(e) }));

let live = null;
let smrLive = null;
let spill = null;

// Recovery chain: delta-encoded from a cursor the worker advances as commits
// land, so each message carries only the new blocks. {error} rides through
// untouched — the panel freezes on it instead of guessing.
let chainFrom = 0;
// The oldest unsettled age per round, sent from the same kind of cursor.
let agesFrom = 0;
// The live memory view; null where it does not apply (extended) and between samples. A read grows
// with n × clients (compact walks every server's §5 windows), so it is taken every memEvery rounds,
// and memEvery doubles while a read costs more than a tenth of the stepping it rides on.
let memEvery = 1, memNext = 0;
function memorySample(round, stepMs) {
  if (round < memNext) return null;
  const t0 = performance.now();
  const m = JSON.parse(smrLive.memory_json());
  if (stepMs > 0 && performance.now() - t0 > 0.1 * stepMs && memEvery < 64) memEvery *= 2;
  memNext = round + memEvery;
  return m.error ? null : m;
}
function agesDelta() {
  const ages = JSON.parse(smrLive.unsettled_ages_json(agesFrom));
  agesFrom += ages.length;
  return ages;
}
function chainDelta() {
  if (!smrLive) return null;
  // wasm-bindgen maps Rust u64 to JS BigInt, like inject's op
  const out = JSON.parse(smrLive.chain_json(BigInt(chainFrom)));
  if (out.error) return out;
  chainFrom = out.m;
  return out;
}

// Certificates and the chain delta are read only when the page's block drawer shows them.
function feeds(m) {
  return {
    certs: m.wantCerts ? smrLive.certs_json() : undefined,
    chain: m.wantChain ? chainDelta() : undefined,
  };
}

onmessage = ev => {
  const m = ev.data;
  ready.then(() => {
    try {
      switch (m.type) {
        case 'run': {
          const t0 = performance.now();
          const json = wasm_bindgen.run_scenario_json(JSON.stringify(m.spec));
          const out = JSON.parse(json);
          if (out.error) {
            postMessage({ type: 'error', error: out.error });
          } else {
            postMessage({ type: 'done', json, ms: Math.round(performance.now() - t0) });
          }
          break;
        }
        case 'live-new': {
          if (live) { live.free(); live = null; }
          live = wasm_bindgen.LiveSim.new_with_mode(JSON.stringify(m.spec), !!m.sticky);
          postMessage({ type: 'live-ready', json: live.trace_json() });
          break;
        }
        case 'live-step': {
          if (!live) throw new Error('no live session');
          const json = live.step(m.fraction);
          const out = JSON.parse(json);
          if (out.error) postMessage({ type: 'error', error: out.error });
          else postMessage({ type: 'live-round', json });
          break;
        }
        case 'live-export': {
          if (!live) throw new Error('no live session');
          postMessage({ type: 'live-scenario', json: live.export_scenario() });
          break;
        }
        case 'live-trace': {
          if (!live) throw new Error('no live session');
          postMessage({ type: 'live-tracejson', json: live.trace_json() });
          break;
        }
        case 'smr-run': {
          const t0 = performance.now();
          const json = wasm_bindgen.run_smr_json(JSON.stringify(m.spec));
          const out = JSON.parse(json);
          if (out.error) {
            postMessage({ type: 'error', error: out.error });
          } else if (out.report.failure) {
            postMessage({
              type: 'smr-failed',
              mode: 'batch',
              report: JSON.stringify(out.report),
              scenario: JSON.stringify(m.spec),
              failure: out.report.failure,
              ms: Math.round(performance.now() - t0),
            });
          } else {
            postMessage({ type: 'smr-done', json, ms: Math.round(performance.now() - t0) });
          }
          break;
        }
        case 'smr-live-new': {
          if (smrLive) { smrLive.free(); smrLive = null; }
          chainFrom = 0;
          agesFrom = 0;
          memEvery = 1; memNext = 0;
          smrLive = wasm_bindgen.LiveSmr.new_with_mode(JSON.stringify(m.spec), !!m.sticky);
          postMessage({ type: 'smr-live-ready', delta: smrLive.report_delta_json(), ages: agesDelta(), memory: memorySample(0, 0) });
          break;
        }
        case 'smr-live-step': {
          if (!smrLive) throw new Error('no smr live session');
          const t0 = performance.now();
          const status = smrLive.step(m.fraction);
          const stepMs = performance.now() - t0;
          const parsed = JSON.parse(status);
          if (parsed.error) {
            postMessage({ type: 'error', error: parsed.error });
          } else if (parsed.failure) {
            const report = smrLive.report_json();
            const scenario = smrLive.export_scenario();
            postMessage({
              type: 'smr-failed',
              mode: 'live',
              status,
              report,
              scenario,
              ages: agesDelta(),
              failure: parsed.failure,
            });
          } else {
            postMessage({ type: 'smr-round', status, delta: smrLive.report_delta_json(), ages: agesDelta(), memory: memorySample(parsed.round, stepMs), ...feeds(m) });
          }
          break;
        }
        // Fast path: one fraction per round, stepped back to back, and the
        // report / certs / chain read once at the end. Stops early on an absorbing
        // status, and a typed failure leaves the same message a single step does.
        case 'smr-live-steps': {
          if (!smrLive) throw new Error('no smr live session');
          let status = null;
          let steps = 0;
          let stop = false;
          const t0 = performance.now();
          for (const fraction of m.fractions) {
            status = smrLive.step(fraction);
            steps++;
            const parsed = JSON.parse(status);
            if (parsed.error) {
              postMessage({ type: 'error', error: parsed.error });
              stop = true;
              break;
            }
            if (parsed.failure) {
              postMessage({
                type: 'smr-failed',
                mode: 'live',
                status,
                report: smrLive.report_json(),
                scenario: smrLive.export_scenario(),
                ages: agesDelta(),
                failure: parsed.failure,
              });
              stop = true;
              break;
            }
            if (parsed.absorbed || parsed.dead !== null) break;
          }
          if (!stop && status !== null) {
            postMessage({ type: 'smr-round', status, steps, delta: smrLive.report_delta_json(), ages: agesDelta(), memory: memorySample(JSON.parse(status).round, performance.now() - t0), ...feeds(m) });
          }
          break;
        }
        case 'smr-live-inject': {
          if (!smrLive) throw new Error('no smr live session');
          // wasm-bindgen maps Rust u64 to JS BigInt
          postMessage({ type: 'smr-injected', quiet: !!m.quiet, json: smrLive.inject(m.client, BigInt(m.op), m.target === null ? undefined : m.target), ...feeds(m) });
          break;
        }
        case 'smr-capture-stale': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-captured', json: smrLive.capture_stale(m.client), certs: smrLive.certs_json() });
          break;
        }
        case 'smr-verify-all': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-verify', json: smrLive.verify_json() });
          break;
        }
        case 'smr-live-traffic': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-traffic-set', json: smrLive.set_traffic(JSON.stringify(m.pmf)) });
          break;
        }
        case 'smr-live-export': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-scenario', json: smrLive.export_scenario() });
          break;
        }
        case 'smr-live-feeds': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-feeds', ...feeds(m) });
          break;
        }
        case 'smr-live-report': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-report', json: smrLive.report_json() });
          break;
        }
        case 'smr-merkle-tree': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-merkle-tree', start: m.start, height: m.height, json: smrLive.merkle_tree_json(BigInt(m.start), m.height) });
          break;
        }
        case 'smr-node-detail': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-node-detail', node: m.node, pinned: !!m.pinned, json: smrLive.node_detail(m.node) });
          break;
        }
        case 'smr-live-block': {
          if (!smrLive) throw new Error('no smr live session');
          postMessage({ type: 'smr-blocked', node: m.node, blocked: !!m.blocked, json: smrLive.set_block(m.node, !!m.blocked) });
          break;
        }
        // The spill reader is Rust: parsing, reconstruction and digest
        // verification all happen inside SpillView, never in JS.
        case 'spill-load': {
          if (spill) { spill.free(); spill = null; }
          try {
            spill = new wasm_bindgen.SpillView(m.text);
          } catch (e) {
            postMessage({ type: 'spill-ready', json: JSON.stringify({ error: String(e) }) });
            break;
          }
          postMessage({ type: 'spill-ready', json: spill.summary_json() });
          break;
        }
        case 'spill-chain': {
          if (!spill) throw new Error('no spill loaded');
          postMessage({
            type: 'spill-chain',
            boundary: m.boundary,
            node: m.node,
            json: spill.chain_json(m.boundary, m.node, 0n),
          });
          break;
        }
      }
    } catch (e) {
      // A Rust panic reaches us as an opaque trap; the hook kept the real
      // message. The engine's memory is unusable afterwards, so drop the
      // session handle and let the main thread respawn.
      const panic = wasm_bindgen.last_panic();
      if (panic) { try { smrLive.free(); } catch (_) { /* already gone */ } smrLive = null; }
      postMessage({ type: 'error', error: String(e), panic: panic || null });
    }
  });
};
