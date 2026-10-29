// Shared row/keyboard helpers (F3): the Enter/Space activation idiom and
// the context-menu viewport clamp, both of which were repeated verbatim
// across components.

// Enter and Space are the two keys that activate a role="button" row.
export function isActivateKey(key: string): boolean {
  return key === 'Enter' || key === ' ';
}

// Build an onkeydown handler that runs `fn` on Enter/Space (a row's
// keyboard activation), suppressing the default (a Space page-scroll).
export function onActivate(fn: () => void) {
  return (e: KeyboardEvent): void => {
    if (isActivateKey(e.key)) {
      e.preventDefault();
      fn();
    }
  };
}

// A context menu is position:fixed at the raw click point: a right-click
// near the bottom/right edge would render off-screen, so after it renders,
// measure it and shift it inside the window bounds.
export function ctxClamp(raw: { x: number; y: number }, el: HTMLElement): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  return {
    x: Math.max(0, Math.min(raw.x, window.innerWidth - r.width - 4)),
    y: Math.max(0, Math.min(raw.y, window.innerHeight - r.height - 4))
  };
}
