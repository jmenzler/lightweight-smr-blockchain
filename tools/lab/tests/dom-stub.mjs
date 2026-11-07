// A just-enough DOM element for driving the lab modules under node:vm.
export class Element {
  constructor(id = '') {
    this.id = id;
    this.value = '';
    this.textContent = '';
    this.innerHTML = '';
    this.children = [];
    this.listeners = {};
    this.dataset = {};
    this.clientWidth = 0;
    this.clientHeight = 0;
    this.classes = new Set();
    this.classList = {
      add: (...xs) => xs.forEach(x => this.classes.add(x)),
      remove: (...xs) => xs.forEach(x => this.classes.delete(x)),
      contains: x => this.classes.has(x),
      toggle: (x, on = !this.classes.has(x)) => {
        on ? this.classes.add(x) : this.classes.delete(x);
        return on;
      },
    };
    this.queries = new Map();
  }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  dispatchEvent(event) { for (const fn of this.listeners[event.type] || []) fn(event); }
  click() { this.dispatchEvent({ type: 'click', target: this, stopPropagation() {} }); }
  querySelector(query) {
    if (!this.queries.has(query)) this.queries.set(query, new Element(query));
    return this.queries.get(query);
  }
  querySelectorAll() { return []; }
  appendChild(child) { this.children.push(child); return child; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 0, height: 0 }; }
  remove() {}
}
