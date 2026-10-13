// @vitest-environment jsdom
// The file-content view: the center-column viewer. Renders the file's actual
// content with line numbers and git-diff gutter marks (added/removed).
// Highlighting is mocked out — this tests the render structure, not shiki.
// Close + switch assert the setChangesView wiring.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import ChangesView from './ChangesView.svelte';
import { mockStore, resetMockStore, setChangesView } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

// Decouple the render test from shiki's WASM engine: one plain (uncolored)
// token per line, so assertions are about the diff structure, not colors.
vi.mock('../lib/highlight', () => ({
  highlightLines: (code: string) => Promise.resolve(code.split('\n').map((l) => [{ content: l }]))
}));

const WS = 'w1';

function seedStatus(): void {
  resetMockStore({
    current: null,
    sessions: {},
    gitStatus: {
      [WS]: {
        branch: 'main',
        modified: ['src/a.ts', 'src/b.ts'],
        untracked: ['notes.md'],
        deleted: []
      }
    }
  });
}

const user = userEvent.setup();

// Open the view on a file with a given current + base (git) content.
function open(p: string, next: string, base: string): void {
  setChangesView({ workspace: WS, path: p, ignored: false });
  mockStore.changesViewNew = next;
  mockStore.changesViewBase = base;
}

function lines(): Element[] {
  return [...document.querySelectorAll('.code .line')];
}

describe('ChangesView', () => {
  beforeEach(() => seedStatus());

  it('renders nothing while no view is open', async () => {
    render(ChangesView);
    await tick();
    expect(screen.queryByText('src/a.ts')).toBeNull();
  });

  it('renders file content with line numbers and diff marks', async () => {
    // base 'one' → new 'two\nthree': the old line is a removed ghost, the two
    // new lines are added. Render order is the interleaved diff order.
    open('src/a.ts', 'two\nthree', 'one');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(3));

    const ls = lines();
    expect(ls.map((l) => l.querySelector('.ln')?.textContent)).toEqual(['', '1', '2']);
    expect(ls.map((l) => l.querySelector('.mk')?.textContent)).toEqual(['−', '+', '+']);
    expect(ls[0]!).toHaveClass('removed');
    expect(ls[1]!).toHaveClass('added');
    expect(ls[2]!).toHaveClass('added');
    expect(ls[0]!.querySelector('.src')?.textContent).toBe('one');
    expect(ls[1]!.querySelector('.src')?.textContent).toBe('two');
  });

  it('shows all context lines when nothing changed', async () => {
    open('src/a.ts', 'a\nb', 'a\nb');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));

    const ls = lines();
    expect(ls.map((l) => l.querySelector('.ln')?.textContent)).toEqual(['1', '2']);
    expect(ls.map((l) => l.querySelector('.mk')?.textContent)).toEqual(['', '']);
    expect(document.querySelector('.code .line.added')).toBeNull();
    expect(document.querySelector('.code .line.removed')).toBeNull();
  });

  it('renders an untracked file as all-added', async () => {
    open('notes.md', 'hello\nworld', '');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));

    expect(lines().every((l) => l.classList.contains('added'))).toBe(true);
  });

  it('shows a note for an empty file', async () => {
    open('src/a.ts', '', '');
    render(ChangesView);
    await vi.waitFor(() => expect(screen.getByText('empty file')).toBeInTheDocument());
  });

  it('lists the changed files in the switcher, the current one on', async () => {
    open('src/a.ts', 'x', 'y');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));

    expect(document.querySelector('.view .title')?.textContent).toBe('src/a.ts');
    const on = screen.getByRole('button', { name: 'a.ts' });
    expect(on).toHaveClass('on');
    expect(screen.getByRole('button', { name: 'b.ts' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'notes.md' })).toBeInTheDocument();
  });

  it('close calls setChangesView with null', async () => {
    open('src/a.ts', 'x', 'y');
    vi.mocked(setChangesView).mockClear();
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));

    await user.click(screen.getByTitle('close (esc)'));
    expect(setChangesView).toHaveBeenCalledWith(null);
  });

  it('switching files keeps the workspace and changes the path', async () => {
    open('src/a.ts', 'x', 'y');
    vi.mocked(setChangesView).mockClear();
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));

    await user.click(screen.getByRole('button', { name: 'b.ts' }));
    expect(setChangesView).toHaveBeenCalledWith({ workspace: WS, path: 'src/b.ts', ignored: false });
  });
});
