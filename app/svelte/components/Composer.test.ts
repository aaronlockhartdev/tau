// @vitest-environment jsdom
// The composer: enter sends (shift+enter breaks the line), the 3-way lane
// selector, stop mode while a turn is running, the leading-`/` command
// dropdown, and the archived-session quiet mode.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta, SkillInfo } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import Composer from './Composer.svelte';
import {
  mockStore,
  resetMockStore,
  seedState,
  send,
  stop
} from '../lib/testing/mock-store.svelte';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte');
  return { ...m, store: m.mockStore };
});

const WS = 'w1';

function meta(id: string, over: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace: WS,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false,
    ...over
  };
}

const skill = (name: string): SkillInfo => ({
  name,
  description: `the ${name} skill`,
  location: `/skills/${name}`,
  model_invocation: true
});

type SeedOver = Partial<{
  model: string | null;
  archived: boolean;
  turn: 'running' | 'idle' | 'starting';
  skills: SkillInfo[];
}>;

function seed(over: SeedOver = {}): SessionState {
  const self = seedState(meta('s1', over.model != null ? { model: over.model } : {}), {
    over: {
      turn: over.turn ?? 'idle',
      archived: over.archived ?? false
    }
  });
  resetMockStore({
    current: 's1',
    sessions: { s1: self },
    skills: { [WS]: over.skills ?? [] }
  });
  return self;
}

beforeEach(() => {
  seed();
});

const user = userEvent.setup();
const PH = 'message — / for commands';

describe('Composer', () => {
  it('an empty field disables the send button', () => {
    render(Composer);
    expect(screen.getByPlaceholderText(PH)).toBeInTheDocument();
    expect(screen.getByTitle('send')).toHaveClass('disabled');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('an empty enter does not send', async () => {
    render(Composer);
    await user.keyboard('{Enter}');
    expect(send).not.toHaveBeenCalled();
  });

  it('enter sends the text on the steering lane and clears the field', async () => {
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, 'hello');
    await user.keyboard('{Enter}');
    expect(send).toHaveBeenCalledWith('hello', 'steering');
    expect(input).toHaveValue('');
  });

  it('shift+enter breaks the line without sending', async () => {
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, 'line one');
    await user.keyboard('{Shift>}{Enter}{/Shift}');
    expect(send).not.toHaveBeenCalled();
    expect(input).toHaveValue('line one\n');
  });

  it('the lane selector changes the next send', async () => {
    render(Composer);
    await user.click(screen.getByRole('button', { name: 'force' }));
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, 'cut in');
    await user.keyboard('{Enter}');
    expect(send).toHaveBeenCalledWith('cut in', 'force');
  });

  it('the lanes dim while the session is not running', () => {
    const { container } = render(Composer);
    expect(container.querySelector('.lanes')).toHaveClass('dim');
  });

  it('while a turn is running, an empty field offers stop', async () => {
    seed({ turn: 'running' });
    render(Composer);
    const btn = screen.getByTitle('stop the in-flight turn');
    expect(btn).toHaveClass('stop');
    await user.click(btn);
    expect(stop).toHaveBeenCalledTimes(1);
  });

  it('while running, a non-empty field sends; force shows the bolt', async () => {
    seed({ turn: 'running' });
    render(Composer);
    await user.click(screen.getByRole('button', { name: 'force' }));
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, 'nudge');
    expect(screen.getByTitle('send')).toHaveTextContent('⚡');
    await user.keyboard('{Enter}');
    expect(send).toHaveBeenCalledWith('nudge', 'force');
  });

  it('a leading slash opens the dropdown with the fixed commands', async () => {
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/');
    const listbox = screen.getByRole('listbox');
    expect(listbox).toHaveTextContent('/model');
    expect(listbox).toHaveTextContent('/help');
  });

  it('typing narrows the dropdown', async () => {
    seed({ skills: [skill('alpha')] });
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/hel');
    const listbox = screen.getByRole('listbox');
    expect(listbox).toHaveTextContent('/help');
    expect(listbox).not.toHaveTextContent('/model');
    expect(listbox).not.toHaveTextContent('/skill:alpha');
  });

  it('arrows navigate and enter completes the selected command', async () => {
    seed({ skills: [skill('alpha')] });
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/');
    // options: /model, /help, /skill:alpha
    await user.keyboard('{ArrowDown}');
    await user.keyboard('{Enter}');
    expect(input).toHaveValue('/');
  });

  it('tab completes a skill suggestion and closes the dropdown', async () => {
    seed({ skills: [skill('alpha'), skill('alphabet')] });
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/skill:alp');
    await user.keyboard('{Tab}');
    expect(input).toHaveValue('/skill:alpha ');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('escape dismisses the dropdown and keeps the text', async () => {
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).toBeNull();
    expect(input).toHaveValue('/');
  });

  it('clicking a suggestion completes it', async () => {
    seed({ skills: [skill('alpha'), skill('beta')] });
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/skill:');
    await user.click(screen.getByRole('option', { name: /skill:alpha/ }));
    expect(input).toHaveValue('/skill:alpha ');
  });

  it('completing /model opens the model menu and clears the field', async () => {
    render(Composer);
    const input = screen.getByPlaceholderText(PH);
    await user.type(input, '/model');
    await user.keyboard('{Enter}');
    expect(input).toHaveValue('');
    expect(mockStore.modelMenuOpen).toBe(true);
  });

  it('the model chip shows the model name and opens the menu', async () => {
    seed({ model: 'acme/x-1' });
    render(Composer);
    expect(screen.getByText('x-1')).toBeInTheDocument();
    await user.click(screen.getByTitle('switch model'));
    expect(mockStore.modelMenuOpen).toBe(true);
  });

  it('the model chip falls back to "no model"', () => {
    render(Composer);
    expect(screen.getByText('no model')).toBeInTheDocument();
  });

  it('an archived session replaces the input with a restore hint', () => {
    seed({ archived: true });
    render(Composer);
    expect(screen.getByText('archived — restore to continue')).toBeInTheDocument();
    expect(screen.queryByPlaceholderText(PH)).toBeNull();
  });
});
