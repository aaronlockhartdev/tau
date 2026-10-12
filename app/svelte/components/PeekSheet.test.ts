// @vitest-environment jsdom
// The peek sheet: a non-modal diff viewer. Renders only while a peek is open;
// the switcher lists the workspace's changed files; close and switch assert
// the setPeek wiring.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import PeekSheet from './PeekSheet.svelte';
import { mockStore, resetMockStore, setPeek } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

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

async function mount(): Promise<void> {
  render(PeekSheet);
  await tick();
}

const user = userEvent.setup();

describe('PeekSheet', () => {
  beforeEach(() => {
    seedStatus();
  });

  it('renders nothing while no peek is open', async () => {
    await mount();
    expect(screen.queryByText('src/a.ts')).toBeNull();
  });

  it('renders the diff with tinted lines and a file switcher', async () => {
    setPeek({ workspace: WS, path: 'src/a.ts' });
    mockStore.peekDiff =
      'diff --git a/src/a.ts b/src/a.ts\n@@ -1,2 +1,2 @@\n-old line\n+new line\n context';
    await mount();
    expect(document.querySelector('.peek .title')?.textContent).toBe('src/a.ts');
    expect(document.querySelector('.diff .al')?.textContent).toBe('+new line');
    expect(document.querySelector('.diff .dl')?.textContent).toBe('-old line');
    expect(document.querySelector('.diff .hh')?.textContent).toBe('@@ -1,2 +1,2 @@');
    // The switcher lists every changed file, the current one marked on.
    const on = screen.getByRole('button', { name: 'a.ts' });
    expect(on).toHaveClass('on');
    expect(screen.getByRole('button', { name: 'b.ts' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'notes.md' })).toBeInTheDocument();
  });

  it('shows a note for a file with no diff', async () => {
    setPeek({ workspace: WS, path: 'notes.md' });
    mockStore.peekDiff = '';
    await mount();
    expect(screen.getByText('no diff — untracked or not in git')).toBeInTheDocument();
  });

  it('close calls setPeek with null', async () => {
    setPeek({ workspace: WS, path: 'src/a.ts' });
    mockStore.peekDiff = '+x';
    vi.mocked(setPeek).mockClear();
    await mount();
    await user.click(screen.getByTitle('close (esc)'));
    expect(setPeek).toHaveBeenCalledWith(null);
  });

  it('switching files keeps the workspace and changes the path', async () => {
    setPeek({ workspace: WS, path: 'src/a.ts' });
    mockStore.peekDiff = '+x';
    vi.mocked(setPeek).mockClear();
    await mount();
    await user.click(screen.getByRole('button', { name: 'b.ts' }));
    expect(setPeek).toHaveBeenCalledWith({ workspace: WS, path: 'src/b.ts' });
  });
});
