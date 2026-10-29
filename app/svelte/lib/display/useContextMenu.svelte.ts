import { tick } from 'svelte';
import { ctxClamp } from './ui';

export interface ContextMenuOptions {
  /** Close the menu (set the anchor to null) plus any extra work. */
  close: () => void;
  /** Return true to keep the menu open for this click. */
  ignoreClick?: (e: MouseEvent) => boolean;
  /** Keep the click-away listener alive while true, even with no menu open. */
  extraActive?: () => boolean;
}

/**
 * The shared right-click menu lifecycle, installed from a component `$effect`:
 * the click-away / Escape listeners and the post-render viewport clamp. The
 * anchor + position state stays component-local (the template's reactivity
 * needs a top-level `$state`), so this takes it and returns the cleanup.
 */
export function useContextMenu(
  ctx: { x: number; y: number } | null,
  menu: { pos: { x: number; y: number }; el: HTMLDivElement | undefined },
  opts: ContextMenuOptions,
): (() => void) | undefined {
  if (!ctx && !opts.extraActive?.()) return;
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') opts.close();
  };
  const onClick = (e: MouseEvent): void => {
    if (opts.ignoreClick?.(e)) return;
    opts.close();
  };
  document.addEventListener('click', onClick);
  document.addEventListener('keydown', onKey);
  void tick().then(() => {
    const el = menu.el;
    if (!ctx || !el) return;
    menu.pos = ctxClamp(ctx, el);
  });
  return () => {
    document.removeEventListener('click', onClick);
    document.removeEventListener('keydown', onKey);
  };
}
