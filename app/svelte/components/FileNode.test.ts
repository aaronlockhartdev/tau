// @vitest-environment jsdom
// One files-tree node: a file row is inert; a dir row toggles its listing
// (fetch on first expand) or re-fetches when the last listing failed.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { FileEntry } from '../lib/protocol';
import FileNode from './FileNode.svelte';
import { mockStore, resetMockStore, retryDirFetch, toggleFileDir } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const WS = 'w1';
const dir = (name: string, path: string): FileEntry => ({ name, path, dir: true, size: 0 });
const file = (name: string, path: string): FileEntry => ({ name, path, dir: false, size: 1 });

beforeEach(() => {
  resetMockStore();
});

describe('FileNode', () => {
  it('a file row renders its name with no chevron', () => {
    render(FileNode, { props: { entry: file('a.txt', '/a.txt'), ws: WS } });
    expect(screen.getByText('a.txt')).toBeInTheDocument();
    expect(document.querySelector('.chev')).toBeDisabled();
  });

  it('clicking an unlisted dir fetches it (toggleFileDir)', async () => {
    const user = userEvent.setup();
    render(FileNode, { props: { entry: dir('src', '/src'), ws: WS } });
    await user.click(screen.getByText('src'));
    expect(toggleFileDir).toHaveBeenCalledWith(WS, '/src');
    expect(retryDirFetch).not.toHaveBeenCalled();
  });

  it('a listed dir renders its children indented', () => {
    mockStore.files = { [WS]: { '/src': [file('main.ts', '/src/main.ts'), dir('lib', '/src/lib')] } };
    render(FileNode, { props: { entry: dir('src', '/src'), ws: WS } });
    expect(screen.getByText('main.ts')).toBeInTheDocument();
    expect(screen.getByText('lib')).toBeInTheDocument();
    // nested node carries the deeper indent
    const kids = document.querySelectorAll('.node');
    expect(kids).toHaveLength(3);
    expect((kids[1] as HTMLElement).style.getPropertyValue('--indent')).toBe('10px');
  });

  it('a listed dir shows the failure note when its fetch failed', () => {
    mockStore.files = { [WS]: { '/src': [] } };
    mockStore.fileErrors = { [WS]: { '/src': 'permission denied' } };
    render(FileNode, { props: { entry: dir('src', '/src'), ws: WS } });
    expect(screen.getByText('failed to list — click the row to retry')).toBeInTheDocument();
  });

  it('clicking a failed dir re-fetches instead of toggling', async () => {
    mockStore.files = { [WS]: { '/src': [] } };
    mockStore.fileErrors = { [WS]: { '/src': 'permission denied' } };
    const user = userEvent.setup();
    render(FileNode, { props: { entry: dir('src', '/src'), ws: WS } });
    await user.click(screen.getByText('src'));
    expect(retryDirFetch).toHaveBeenCalledWith(WS, '/src');
    expect(toggleFileDir).not.toHaveBeenCalled();
  });

  it('clicking a file does nothing', async () => {
    const user = userEvent.setup();
    render(FileNode, { props: { entry: file('a.txt', '/a.txt'), ws: WS } });
    await user.click(screen.getByText('a.txt'));
    expect(toggleFileDir).not.toHaveBeenCalled();
    expect(retryDirFetch).not.toHaveBeenCalled();
  });
});
