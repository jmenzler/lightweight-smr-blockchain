// Header and shared chrome: host warning, mode switch, dropdown popovers.
import { $ } from './state.js';
import { setView } from './render.js';

if (globalThis.location?.hostname.endsWith('github.io')) $('hostWarn').classList.remove('hidden');

for (const b of document.querySelectorAll('#modeTabs .mt')) {
  b.addEventListener('click', () => setView(b.dataset.mode));
}

for (const pop of document.querySelectorAll('.pop')) {
  pop.querySelector('.popBtn').addEventListener('click', e => {
    e.stopPropagation();
    for (const o of document.querySelectorAll('.pop.open')) if (o !== pop) o.classList.remove('open');
    pop.classList.toggle('open');
  });
}
for (const item of document.querySelectorAll('.pop .menuItem')) {
  item.addEventListener('click', () => item.closest('.pop').classList.remove('open'));
}
document.addEventListener('click', e => {
  if (!e.target.closest('.pop')) for (const o of document.querySelectorAll('.pop.open')) o.classList.remove('open');
});

$('aboutBtn').addEventListener('click', () => $('aboutPanel').classList.toggle('hidden'));
$('aboutClose').addEventListener('click', () => $('aboutPanel').classList.add('hidden'));
window.addEventListener('keydown', e => { if (e.key === 'Escape') $('aboutPanel').classList.add('hidden'); });
