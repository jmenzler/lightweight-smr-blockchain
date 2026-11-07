// bootstrap: worker dispatch + initial paint
import { S, $ } from './state.js';
import { onMessage, respawn } from './engine.js';
import { draw } from './render.js';
import { onBatchDone, onLiveReady, onLiveRound, onLiveScenario, onLiveTraceJson } from './live.js';
import './controls.js';
import './tooltip.js';
import './chrome.js';
import { onSmrDone, onSmrReady, onSmrRound, onSmrInjected, onSmrScenario, onSmrReport, onSmrFeeds, onSmrTrafficSet, onSmrCaptured, onSmrVerify, onSmrNodeDetail, onSmrBlocked, onSmrFailed, onSmrAbort } from './smr.js';
import { onSpillReady, onSpillChain } from './spill.js';
import { onMerkleTree } from './smr-drawer.js';

onMessage(m => {
  if (m.type === 'ready') { S.engine = 'wasm'; $('engineChip').textContent = 'engine: Rust WASM — canonical seeds'; $('engineChip').className = 'chip chip-ok engineChip'; }
  else if (m.type === 'initfail') { S.engine = 'failed'; $('engineChip').textContent = 'engine failed: ' + m.error + ' — serve via http, not file://'; $('engineChip').className = 'chip chip-dead engineChip'; }
  else if (m.type === 'done') { onBatchDone(m); }
  else if (m.type === 'live-ready') { onLiveReady(m); }
  else if (m.type === 'live-round') { onLiveRound(JSON.parse(m.json)); }
  else if (m.type === 'live-scenario') { onLiveScenario(m); }
  else if (m.type === 'live-tracejson') { onLiveTraceJson(m); }
  else if (m.type === 'smr-done') { onSmrDone(m); }
  else if (m.type === 'smr-failed') { onSmrFailed(m); }
  else if (m.type === 'smr-live-ready') { onSmrReady(m); }
  else if (m.type === 'smr-round') { onSmrRound(m); }
  else if (m.type === 'smr-injected') { onSmrInjected(m); }
  else if (m.type === 'smr-scenario') { onSmrScenario(m); }
  else if (m.type === 'smr-report') { onSmrReport(m); }
  else if (m.type === 'smr-feeds') { onSmrFeeds(m); }
  else if (m.type === 'smr-traffic-set') { onSmrTrafficSet(m); }
  else if (m.type === 'smr-captured') { onSmrCaptured(m); }
  else if (m.type === 'smr-verify') { onSmrVerify(m); }
  else if (m.type === 'smr-node-detail') { onSmrNodeDetail(m); }
  else if (m.type === 'smr-blocked') { onSmrBlocked(m); }
  else if (m.type === 'smr-merkle-tree') { onMerkleTree(m); }
  else if (m.type === 'spill-ready') { onSpillReady(m); }
  else if (m.type === 'spill-chain') { onSpillChain(m); }
  else if (m.type === 'error') {
    if (S.live) { S.live.playing = false; S.live.awaiting = false; }
    if (S.smr && S.smr.live) { S.smr.live.playing = false; S.smr.live.awaiting = false; }
    $('runStatus').textContent = 'error: ' + m.error;
    $('smrStatus').textContent = 'error: ' + m.error;
    if (m.panic) {
      // Post-trap wasm memory is untrustworthy, so both sessions die with it.
      onSmrAbort(m);
      S.live = null;
      respawn();
    }
  }
});

window.addEventListener('resize', draw);
draw();
