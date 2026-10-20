// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Markdown from './Markdown.svelte';

// The highlighter is the async shiki boundary; the component under test is
// the two-phase render: plain base first, colored after.
const highlightHtml = vi.hoisted(() =>
  vi.fn((code: string, lang: string) => Promise.resolve(`<i data-lang="${lang}">${code}</i>`))
);
vi.mock('../lib/highlight', () => ({ highlightHtml }));

afterEach(() => {
  vi.clearAllMocks();
});

describe('Markdown', () => {
  it('renders the plain base synchronously, then upgrades with highlighter output', async () => {
    const { container } = render(Markdown, { props: { text: '```rust\nlet x = 1;\n```' } });
    expect(container.querySelector('pre.code')).not.toBeNull();
    await vi.waitFor(() => {
      expect(container.querySelector('i[data-lang="rust"]')).not.toBeNull();
    });
    expect(container.querySelector('pre.code')).toBeNull();
    expect(screen.getByText('let x = 1;')).toBeInTheDocument();
  });

  it('leaves an unlabeled fence plain', async () => {
    const { container } = render(Markdown, { props: { text: '```\nplain\n```' } });
    await vi.waitFor(() => expect(highlightHtml).not.toHaveBeenCalled());
    expect(container.querySelector('pre.code')).not.toBeNull();
  });

  it('drops a late highlight for a stale text', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((r) => {
      release = r;
    });
    highlightHtml.mockImplementationOnce(
      (code: string, lang: string) =>
        gate.then(() => Promise.resolve(`<i data-lang="${lang}">${code}</i>`))
    );
    const { container, rerender } = render(Markdown, { props: { text: '```rust\nfn a() {}\n```' } });
    rerender({ text: 'plain **body**' });
    release?.();
    await vi.waitFor(() => expect(container.querySelector('p')).not.toBeNull());
    expect(container.querySelector('i[data-lang]')).toBeNull();
  });
});
