// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke, mockListen } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockListen: vi.fn()
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) as Promise<unknown> }));
vi.mock('@tauri-apps/api/event', () => ({ listen: (...args: unknown[]) => mockListen(...args) as Promise<() => void> }));

import { command, isTauri, onEvents, type Command } from './protocol';
import { BusyError } from './errors';

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockReset();
});

describe('command', () => {
  it('forwards the command to the tau_command invoke', async () => {
    mockInvoke.mockResolvedValue({ kind: 'none' });
    const out = await command({ type: 'workspace_list' });
    expect(out).toEqual({ kind: 'none' });
    expect(mockInvoke).toHaveBeenCalledWith('tau_command', {
      command: { type: 'workspace_list' } satisfies Command
    });
  });

  it('wraps a string rejection in an Error carrying the message', async () => {
    mockInvoke.mockRejectedValue('plain failure');
    await expect(command({ type: 'workspace_list' })).rejects.toThrow('plain failure');
  });

  it('wraps a serialized core error: the message field wins', async () => {
    mockInvoke.mockRejectedValue({ message: 'provider 4xx', what: 'x' });
    await expect(command({ type: 'workspace_list' })).rejects.toThrow('provider 4xx');
  });

  it('wraps a not_found rejection: the what field is the message', async () => {
    mockInvoke.mockRejectedValue({ what: 'session s1' });
    await expect(command({ type: 'workspace_list' })).rejects.toThrow('session s1');
  });

  it('a busy rejection is a BusyError, so a caller branches without parsing prose', async () => {
    mockInvoke.mockRejectedValue({ kind: 'busy', what: 'session s1 is running' });
    await expect(command({ type: 'workspace_list' })).rejects.toThrow(BusyError);
  });

  it('a not_found rejection is not a BusyError (only busy branches)', async () => {
    mockInvoke.mockRejectedValue({ kind: 'not_found', what: 'session s1' });
    const err = await command({ type: 'workspace_list' }).catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(BusyError);
    expect((err as Error).message).toBe('session s1');
  });

  it('stringifies a rejection with neither field', async () => {
    mockInvoke.mockRejectedValue({ code: 7 });
    await expect(command({ type: 'workspace_list' })).rejects.toThrow('{"code":7}');
  });
});

describe('onEvents', () => {
  it('listens on tau://event and hands the payload to the callback', async () => {
    let cb: ((events: unknown[]) => void) | null = null;
    mockListen.mockImplementation((topic: string, handler: (e: { payload: unknown[] }) => void) => {
      expect(topic).toBe('tau://event');
      cb = (events) => handler({ payload: events });
      return () => {};
    });
    const got: unknown[] = [];
    const unlisten = await onEvents((evts) => got.push(...evts));
    cb!([{ type: 'system', workspace: 'w', session: null, kind: { kind: 'provider_changed' } }]);
    expect(got).toHaveLength(1);
    unlisten();
  });
});

describe('isTauri', () => {
  it('is true inside the Tauri window', () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    expect(isTauri()).toBe(true);
  });

  it('is false in a plain browser', () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    expect(isTauri()).toBe(false);
  });
});
