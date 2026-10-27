// @vitest-environment jsdom
// #195: a bare <a href="https://..."> in rendered markdown navigates the
// Tauri webview away (the app wedges). The fix is one delegated document
// click handler: http/https links open externally via the opener plugin,
// everything else (mailto, #anchor, relative) keeps its native behavior.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import CardShell from './CardShell.svelte';
import type { Shell } from '../lib/presentation';

// The real module imports the Tauri opener plugin; the tests spy on it.
const openUrl = vi.fn((_u: string) => Promise.resolve());
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (u: string) => openUrl(u) }));

function cardWithLink(): Extract<Shell, { kind: 'card' }> {
  return {
    kind: 'card',
    header: null,
    cls: null,
    openKey: null,
    collapseWhenClosed: false,
    sections: [{ type: 'md', text: 'see [the docs](https://example.com/a)' }],
    reveal: null,
    expando: null,
    intmark: null
  };
}

function clickAnchor(href: string, el: Element): { prevented: boolean } {
  const a = document.createElement('a');
  a.href = href;
  el.appendChild(a);
  const e = new MouseEvent('click', { bubbles: true, cancelable: true });
  a.dispatchEvent(e);
  return { prevented: e.defaultPrevented };
}

describe('externalLinks (#195)', () => {
  beforeEach(() => {
    vi.resetModules();
    openUrl.mockClear();
    // The handler only activates inside the Tauri webview.
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
  });

  it('intercepts an http link in rendered markdown: opens externally, no webview navigation', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const { prevented } = clickAnchor('https://example.com/a', document.body);
    expect(prevented).toBe(true);
    expect(openUrl).toHaveBeenCalledWith('https://example.com/a');
    detach();
  });

  it('intercepts an https link too', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    clickAnchor('https://example.com/b', document.body);
    expect(openUrl).toHaveBeenCalledWith('https://example.com/b');
    detach();
  });

  it('leaves mailto links alone (native mail client handling)', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const { prevented } = clickAnchor('mailto:a@example.com', document.body);
    expect(prevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    detach();
  });

  it('leaves #anchor links alone (in-page navigation)', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const { prevented } = clickAnchor('#section', document.body);
    expect(prevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    detach();
  });

  it('leaves relative links alone', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const { prevented } = clickAnchor('/some/page', document.body);
    expect(prevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    detach();
  });

  it('ignores clicks that are not on a link', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const div = document.createElement('div');
    document.body.appendChild(div);
    div.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(openUrl).not.toHaveBeenCalled();
    detach();
  });

  it('stays inert outside Tauri (plain browser demo/dev)', async () => {
    delete (window as unknown as Record<string, unknown>)['__TAURI_INTERNALS__'];
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    const { prevented } = clickAnchor('https://example.com/a', document.body);
    expect(prevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    detach();
  });

  it('detach removes the listener', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    detach();
    const { prevented } = clickAnchor('https://example.com/a', document.body);
    expect(prevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });
});

describe('CardShell markdown links (#195)', () => {
  beforeEach(() => {
    openUrl.mockClear();
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
  });

  it('renders the markdown link as an anchor (the handler is what tames it)', () => {
    render(CardShell, {
      props: {
        shell: cardWithLink(),
        open: { tool: false, output: false, skill: false, obs: false, think: false },
        setOpen: () => {},
        onKey: (fn: () => void) => (e: KeyboardEvent) => e.key === 'Enter' && fn()
      }
    });
    const a = screen.getByText('the docs').closest('a');
    expect(a).not.toBeNull();
    expect(a?.getAttribute('href')).toBe('https://example.com/a');
  });

  it('a click on the markdown link opens externally instead of navigating the webview', async () => {
    const { attachExternalLinkHandler } = await import('../lib/externalLinks');
    const detach = attachExternalLinkHandler();
    render(CardShell, {
      props: {
        shell: cardWithLink(),
        open: { tool: false, output: false, skill: false, obs: false, think: false },
        setOpen: () => {},
        onKey: (fn: () => void) => (e: KeyboardEvent) => e.key === 'Enter' && fn()
      }
    });
    const a = screen.getByText('the docs').closest('a');
    const e = new MouseEvent('click', { bubbles: true, cancelable: true });
    a?.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    expect(openUrl).toHaveBeenCalledWith('https://example.com/a');
    detach();
  });

  it('the handler module exposes attach/detach (App.svelte wires it in onMount/onDestroy)', async () => {
    const mod = await import('../lib/externalLinks');
    expect(typeof mod.attachExternalLinkHandler).toBe('function');
  });
});
