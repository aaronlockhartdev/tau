// @vitest-environment jsdom
// The expanded tool output: edit renders as a colored git-style diff (the
// core's bounded-diff contract), read/write as highlighted code, the rest as a
// plain pre. Highlighting is mocked out — this tests the render structure,
// not shiki.
import { describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import ToolOutput from './ToolOutput.svelte';

// Decouple from shiki's WASM engine: one plain (uncolored) token per line.
vi.mock('../lib/highlight', () => ({
  highlightLines: (code: string) => Promise.resolve(code.split('\n').map((l) => [{ content: l }]))
}));

function codeLines(): Element[] {
  return [...document.querySelectorAll('.to-code .line')];
}

describe('ToolOutput', () => {
  it('renders an edit output as a colored git-style diff', async () => {
    const text = 'edited src/a.ts (lines 1–2 of 3)\n-old\n+ abc new\n ctx';
    render(ToolOutput, { props: { text, name: 'edit', path: 'src/a.ts' } });
    await vi.waitFor(() => expect(codeLines().length).toBe(3));

    const ls = codeLines();
    expect(ls.map((l) => l.querySelector('.mk')?.textContent)).toEqual(['−', '+', '']);
    expect(ls[0]!).toHaveClass('removed');
    expect(ls[1]!).toHaveClass('added');
    expect(ls[2]!).not.toHaveClass('removed');
    expect(document.querySelector('.to-hd')?.textContent).toBe('edited src/a.ts (lines 1–2 of 3)');
  });

  it('renders the edit truncation notice when present', async () => {
    const text = 'edited a.ts (lines 1–2 of 40)\n+ abc x\n… 38 more lines changed';
    render(ToolOutput, { props: { text, name: 'edit', path: 'a.ts' } });
    await vi.waitFor(() =>
      expect(document.querySelector('.to-trunc')?.textContent).toBe('… 38 more lines changed')
    );
  });

  it('renders a no-change edit as just its header (empty body)', async () => {
    render(ToolOutput, {
      props: { text: 'edited a.ts (no change)', name: 'edit', path: 'a.ts' }
    });
    await vi.waitFor(() =>
      expect(document.querySelector('.to-hd')?.textContent).toBe('edited a.ts (no change)')
    );
    expect(codeLines().length).toBe(0);
  });

  it('renders a read output as highlighted code with no diff marks', async () => {
    render(ToolOutput, {
      props: { text: 'const a = 1;\nconst b = 2;', name: 'read', path: 'a.ts' }
    });
    await vi.waitFor(() => expect(codeLines().length).toBe(2));

    expect(codeLines().map((l) => l.querySelector('.mk')?.textContent)).toEqual(['', '']);
    expect(document.querySelector('.to-code .line .src')?.textContent).toBe('const a = 1;');
  });

  it('renders a non-code tool as a plain pre, not the code view', () => {
    render(ToolOutput, { props: { text: 'plain output', name: 'recall' } });
    expect(document.querySelector('pre')?.textContent).toBe('plain output');
    expect(document.querySelector('.to-code')).toBeNull();
  });
});
