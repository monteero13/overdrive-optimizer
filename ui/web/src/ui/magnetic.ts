// "Magnetic" CTA: the button leans toward the pointer when it is near.
// Disabled with reduced motion and on disabled buttons.
export function bindMagnetic(root: HTMLElement, reduced: () => boolean): void {
  const RADIUS = 90;
  const MAX = 7;
  let current: HTMLElement | null = null;

  root.addEventListener('pointermove', (e) => {
    if (reduced()) return;
    const el = root.querySelector<HTMLElement>('[data-magnetic]:not([disabled])');
    if (current && current !== el) current.style.transform = '';
    current = el;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const dx = e.clientX - cx;
    const dy = e.clientY - cy;
    const dist = Math.hypot(dx, dy) - Math.max(r.width, r.height) / 2;
    if (dist > RADIUS) {
      el.style.transform = '';
      return;
    }
    const pull = 1 - Math.max(0, dist) / RADIUS;
    const len = Math.hypot(dx, dy) || 1;
    el.style.transform = `translate(${((dx / len) * MAX * pull).toFixed(2)}px, ${((dy / len) * MAX * pull).toFixed(2)}px)`;
  });
  root.addEventListener('pointerleave', () => {
    if (current) current.style.transform = '';
  });
}
