// @vitest-environment jsdom
// The file viewer (open-files model): a per-workspace tab strip of open files
// plus the active file's content with line numbers and git-diff gutter marks.
// Highlighting is mocked — this tests the render structure, not shiki. The
// tab / close / back controls assert the store wiring; the empty / binary /
// unreadable notes and the highlight-failure fallback are the A3 / A1 cases.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import ChangesView from './ChangesView.svelte';
import { highlightLines } from '../lib/display/highlight';
import {
  closeFile,
  resetMockStore,
  selectFile,
  setCenterMode
} from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

vi.mock('../lib/display/highlight', () => ({
  highlightLines: vi.fn((code: string) => Promise.resolve(code.split('\n').map((l) => [{ content: l }])))
}));

const WS = 'w1';
const user = userEvent.setup();

// Open a single file with a given current + base (git) content.
function seedOpen(p: string, next: string, base: string): void {
  resetMockStore({
    openFiles: { [WS]: [{ path: p, ignored: false, size: next.length }] },
    activeFile: { workspace: WS, path: p },
    activeFileNew: next,
    activeFileBase: base,
    activeFileError: null
  });
}

function seedTwo(): void {
  resetMockStore({
    openFiles: {
      [WS]: [
        { path: 'src/a.ts', ignored: false, size: 1 },
        { path: 'src/b.ts', ignored: false, size: 1 }
      ]
    },
    activeFile: { workspace: WS, path: 'src/a.ts' },
    activeFileNew: 'x',
    activeFileBase: 'y',
    activeFileError: null
  });
}

function lines(): Element[] {
  return [...document.querySelectorAll('.code .line')];
}

describe('ChangesView', () => {
  beforeEach(() => resetMockStore());

  it('renders nothing while no file is open', async () => {
    render(ChangesView);
    await tick();
    expect(screen.queryByText('src/a.ts')).toBeNull();
  });

  it('renders file content with line numbers and diff marks', async () => {
    // base 'one' → new 'two\nthree': the old line is a removed ghost, the two
    // new lines are added. Render order is the interleaved diff order.
    seedOpen('src/a.ts', 'two\nthree', 'one');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(3));

    const ls = lines();
    expect(ls.map((l) => l.querySelector('.ln')?.textContent)).toEqual(['', '1', '2']);
    expect(ls.map((l) => l.querySelector('.mk')?.textContent)).toEqual(['−', '+', '+']);
    expect(ls[0]!).toHaveClass('removed');
    expect(ls[1]!).toHaveClass('added');
    expect(ls[0]!.querySelector('.src')?.textContent).toBe('one');
    expect(ls[1]!.querySelector('.src')?.textContent).toBe('two');
  });

  it('does not diff the last line when only the trailing newline differs', async () => {
    seedOpen('src/a.ts', 'a\nb', 'a\nb\n'); // same lines; base carries a final newline
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));
    expect(document.querySelector('.code .line.added')).toBeNull();
    expect(document.querySelector('.code .line.removed')).toBeNull();
  });

  it('sizes the line-number gutter to the widest line number', async () => {
    const big = Array.from({ length: 1000 }, (_, i) => `l${i}`).join('\n');
    seedOpen('src/big.ts', big, big); // 1000 lines -> 4-digit numbers
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(1000));
    expect(document.querySelector('.code')!.getAttribute('style')).toContain('--lnw: 4');
  });

  it('shows all context lines when nothing changed', async () => {
    seedOpen('src/a.ts', 'a\nb', 'a\nb');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));
    expect(document.querySelector('.code .line.added')).toBeNull();
    expect(document.querySelector('.code .line.removed')).toBeNull();
  });

  it('renders an untracked file as all-added', async () => {
    seedOpen('notes.md', 'hello\nworld', '');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));
    expect(lines().every((l) => l.classList.contains('added'))).toBe(true);
  });

  it('shows a note for an empty file', async () => {
    seedOpen('src/a.ts', '', '');
    render(ChangesView);
    await vi.waitFor(() => expect(screen.getByText('empty file')).toBeInTheDocument());
  });

  it('shows the binary reason carried by the store', async () => {
    resetMockStore({
      openFiles: { [WS]: [{ path: 'a.bin', ignored: false, size: 128 }] },
      activeFile: { workspace: WS, path: 'a.bin' },
      activeFileNew: null,
      activeFileBase: null,
      activeFileError: 'binary file · 128 bytes'
    });
    render(ChangesView);
    await vi.waitFor(() => expect(screen.getByText('binary file · 128 bytes')).toBeInTheDocument());
  });

  it('shows the read error carried by the store', async () => {
    resetMockStore({
      openFiles: { [WS]: [{ path: 'gone.ts', ignored: false, size: 0 }] },
      activeFile: { workspace: WS, path: 'gone.ts' },
      activeFileNew: null,
      activeFileBase: null,
      activeFileError: 'cannot read: reading gone.ts: No such file'
    });
    render(ChangesView);
    await vi.waitFor(() =>
      expect(screen.getByText('cannot read: reading gone.ts: No such file')).toBeInTheDocument()
    );
  });

  it('falls back to plain lines when highlighting rejects (never sticks)', async () => {
    vi.mocked(highlightLines).mockRejectedValueOnce(new Error('wasm init failed'));
    seedOpen('src/a.ts', 'one\ntwo', 'one\ntwo');
    render(ChangesView);
    await vi.waitFor(() => expect(lines().length).toBe(2));
    expect(screen.queryByText('loading…')).toBeNull();
  });

  it('lists the open files as tabs, the active one on', async () => {
    seedTwo();
    render(ChangesView);
    await tick();
    const tabs = [...document.querySelectorAll('.tab')];
    expect(tabs).toHaveLength(2);
    expect(tabs[0]!).toHaveClass('on');
    expect(tabs[1]!).not.toHaveClass('on');
  });

  it('clicking a tab calls selectFile with its path', async () => {
    seedTwo();
    vi.mocked(selectFile).mockClear();
    render(ChangesView);
    await tick();
    await user.click(screen.getByRole('button', { name: 'b.ts' }));
    expect(selectFile).toHaveBeenCalledWith(WS, 'src/b.ts');
  });

  it('close calls closeFile with the tab path', async () => {
    seedOpen('src/a.ts', 'x', 'y');
    vi.mocked(closeFile).mockClear();
    render(ChangesView);
    await tick();
    await user.click(screen.getByTitle('close'));
    expect(closeFile).toHaveBeenCalledWith(WS, 'src/a.ts');
  });

  it('back-to-chat calls setCenterMode with chat', async () => {
    seedOpen('src/a.ts', 'x', 'y');
    vi.mocked(setCenterMode).mockClear();
    render(ChangesView);
    await tick();
    await user.click(screen.getByTitle('back to transcript'));
    expect(setCenterMode).toHaveBeenCalledWith(WS, 'chat');
  });
});
