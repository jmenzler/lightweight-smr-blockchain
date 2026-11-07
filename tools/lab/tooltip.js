// Instant tooltips: no hover delay, styled like the dropdowns. Any element with
// data-tip (or a legacy title, which is moved to data-tip on first hover so the
// native delayed bubble never shows) gets one; chart hovers call showTip/hideTip.
const box = document.createElement('div');
box.className = 'tipBox';
document.body.appendChild(box);
let current = null;

export function showTip(html, x, y) {
  box.innerHTML = html;
  box.style.display = 'block';
  const r = box.getBoundingClientRect();
  box.style.left = Math.max(6, Math.min(x + 12, window.innerWidth - r.width - 6)) + 'px';
  box.style.top = (y + 18 + r.height > window.innerHeight ? y - r.height - 10 : y + 18) + 'px';
}
export function hideTip() {
  box.style.display = 'none';
  current = null;
}

export const escapeHtml = s => String(s).replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);

document.addEventListener('mouseover', e => {
  const el = e.target.closest ? e.target.closest('[data-tip],[title]') : null;
  if (!el) { hideTip(); return; }
  if (el === current) return;
  if (el.hasAttribute('title')) {
    el.setAttribute('data-tip', el.getAttribute('title'));
    el.removeAttribute('title');
  }
  const text = el.getAttribute('data-tip');
  if (!text) { hideTip(); return; }
  current = el;
  const r = el.getBoundingClientRect();
  showTip(escapeHtml(text), r.left, r.bottom - 14);
});
