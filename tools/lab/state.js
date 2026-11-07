// shared state + tiny helpers — no dependencies
export const PALETTE = ['#b8862e', '#269c86', '#6b8ae8', '#c05a9e', '#79a53f', '#9a76de', '#b26a41'];
export const BOT = '#4b5563';
export const BLOCKED = '#f7768e';

// traffic arrivals carry reserved high client ids (engine AUTO_CLIENT_BASE)
export const AUTO_CLIENT_BASE = 2147483648;

export const S = { trace: null, t: 0, playing: false, speed: 1, sel: null, edgeMode: 'selected', view: 'single', series: null, rowOrder: null, layout: null, heatStep: 1, engine: 'loading', live: null,
  // single-value session: captured config, strategy program, speed, whether the worker still holds a stopped live session
  sv: { ctx: null, pending: null, handle: false, speed: 1, program: null, srvOpen: true, end: null } };
export const $ = id => document.getElementById(id);
export const clamp01 = x => Math.min(Math.max(x, 0), 1);

export const colorOf = v => v === null ? BOT : PALETTE[Number(v) % PALETTE.length];
export const statesAt = t => t === 0 ? S.trace.initial : S.trace.rounds[t - 1].states;
export const roundInfo = t => t >= 1 ? S.trace.rounds[t - 1] : null;
