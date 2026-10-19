// #195: rendered markdown links used to navigate the Tauri webview (the app
// wedged). One delegated document handler: http/https open externally via
// the opener plugin; everything else (mailto, #anchor, relative) keeps its
// native behavior.
import { openUrl } from '@tauri-apps/plugin-opener';
import { isTauri } from './protocol';

export function attachExternalLinkHandler(): () => void {
  const on = (e: MouseEvent) => {
    if (!isTauri()) return;
    const a = (e.target as Element | null)?.closest('a[href]');
    // Classify on the raw href attribute: a.href resolves #anchor and
    // relative links to absolute http URLs.
    const raw = a?.getAttribute('href') ?? '';
    if (!/^https?:\/\//.test(raw)) return;
    const href = a instanceof HTMLAnchorElement ? a.href : '';
    e.preventDefault();
    void openUrl(href);
  };
  document.addEventListener('click', on);
  return () => document.removeEventListener('click', on);
}
