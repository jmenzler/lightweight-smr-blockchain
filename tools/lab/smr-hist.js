// Small SVG histograms for the run summary and the block drawer: integer values binned over
// [lo, hi] (out-of-range values clamp into the end bins), optional vertical marks and x ticks.
const MONO = 'ui-monospace, Menlo, monospace';

export function histSvg(values, { W, H, lo, hi, bin = 1, color = '#aeb6c0', marks = [], ticks = [], xlab = v => String(v), tip = null }) {
  const nb = Math.max(1, Math.ceil((hi - lo + 1) / bin));
  const counts = new Array(nb).fill(0);
  for (const v of values) counts[Math.floor((Math.min(Math.max(v, lo), hi) - lo) / bin)]++;
  const mx = Math.max(1, ...counts), PB = 14, bw = W / nb;
  const X = v => ((v - lo) / (hi - lo + 1)) * W;
  let s = '';
  counts.forEach((c, k) => {
    if (!c) return;
    const h = Math.max(1.5, (c / mx) * (H - PB - 4));
    const v = lo + k * bin;
    const label = bin > 1 ? `${xlab(v)}…${xlab(v + bin - 1)}` : xlab(v);
    s += `<rect x="${(k * bw + 0.5).toFixed(1)}" y="${(H - PB - h).toFixed(1)}" width="${Math.max(bw - 1, 1).toFixed(1)}" height="${h.toFixed(1)}" fill="${typeof color === 'function' ? color(v) : color}"` +
      (tip ? ` data-tip="${tip(label, c)}"` : '') + '/>';
  });
  for (const [v, col, dash] of marks) {
    s += `<line x1="${X(v).toFixed(1)}" y1="2" x2="${X(v).toFixed(1)}" y2="${H - PB}" stroke="${col}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`;
  }
  for (const v of ticks) {
    s += `<text x="${(X(v) + bw / 2).toFixed(1)}" y="${H - 3}" text-anchor="middle" fill="#8b95a1" font-size="9" font-family="${MONO}">${xlab(v)}</text>`;
  }
  return `<svg class="histSvg" width="${W}" height="${H}">${s}</svg>`;
}

// |displacement| colours shared by the summary and the order tab; 10+ is amber because white
// marks the selected command.
export const dispColor = d => {
  const a = Math.abs(d);
  return a === 0 ? '#3dd6c4' : a <= 3 ? '#5d7bb5' : a <= 9 ? '#a99cf0' : '#e0af68';
};
export const signedInt = v => (v > 0 ? '+' : v < 0 ? '−' : '') + Math.abs(v);
