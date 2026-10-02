// Mock leg, feature 4: a scripted subagent spawn through the real core —
// the parent session creates a task, spawns a child, and the child runs
// its scripted turns against the mock. The GUI-side contract (what the
// retired replay leg's dogfood pair covered): the child session appears
// in the left-pane tree and the parent transcript carries the spawn
// entry, in send order (#50).
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  ensureWorkspace,
  newSessionViaUI,
  readDom,
  readPanes,
  readStore,
  selectModel,
  uiSend,
  waitSettle
} from './support/helpers.mjs';

// The shared context travels from the config's onPrepare over the worker's
// inherited environment.
const ws = process.env.TAU_E2E_WS_MOCK;
const artifacts = process.env.TAU_E2E_ARTIFACTS ?? path.join('..', 'target', 'e2e');
if (!ws) throw new Error('mock context unset — the wdio config did not run onPrepare');

// The parent scenario's marker (fixtures/e2e-mocks/e2e-subagent-parent.json);
// the child session is claimed by the shared acceptance-subagent-child
// scenario through its fixed system prompt, so no marker reaches it.
const MARKER = 'e2e-subagent-turn';

describe('mock E2E: a scripted subagent spawn renders parent and child', () => {
  let failures = 0;
  afterEach(async function () {
    if (this.currentTest?.err) {
      failures++;
      try {
        const log = await browser.execute(() => (window.__evlog ?? []).slice(-60));
        console.log(`diag: evlog @ ${this.currentTest?.title}: ${JSON.stringify(log)}`);
      } catch {
        // the app may be gone; the screenshot below carries the diagnosis
      }
      await browser
        .saveScreenshot(path.join(artifacts, `failure-subagent-${String(failures).padStart(2, '0')}.png`))
        .catch(() => {});
    }
  });

  it('boot: the webview loads and the app module graph ran', async () => {
    await bootCheck();
  });

  it('a fresh session is created through the LeftPane .new button', async () => {
    await ensureWorkspace(ws);
    const sid = await newSessionViaUI();
    expect(sid).toBeTruthy();
  });

  it('the model menu switches the session to mock/mock-model-2', async () => {
    // The mock workspace's provider offers twin models; the switch is the
    // real menu interaction (the mock server ignores the model id).
    await selectModel('mock/mock-model-2');
  });

  it('the send walks the scripted spawn: task, subagent_spawn, text', async () => {
    // One Composer send runs the whole scripted turn (task_create,
    // subagent_spawn, then the text reply); the settle bar is the
    // deadline, not a one-shot read.
    await uiSend(`${MARKER} round 1`);
    await waitSettle(60000);
    const s = await readStore();
    const i = s.entryKinds.indexOf('subagent');
    const u = s.entryKinds.indexOf('user');
    check(
      'the parent transcript carries the subagent entry after the user entry (send order)',
      u >= 0 && i > u,
      JSON.stringify(s.entryKinds)
    );
    // The spawn entry's handle is `{parent session id}-1` (spawn.rs): the
    // parent's view of its first child.
    check(
      'the spawn entry carries the deterministic child handle',
      s.subagentHandles[0] === `${s.current}-1`,
      JSON.stringify(s.subagentHandles)
    );
  });

  it('the spawn card renders in the parent transcript', async () => {
    // Poll the DOM: the card lands when the snapshot applies, and a
    // one-shot read raced that in the pre-#48 shape.
    const d = await waitUntil(
      readDom,
      (d) => d.hasSpawnCard,
      15000,
      'the spawn card in the parent transcript'
    );
    check('the DOM shows the spawn card in the parent transcript', d.hasSpawnCard, 'spawn label present');
  });

  it('the child session appears in the left-pane tree', async () => {
    // The child's title is a random adjective-noun pair (spawn.rs), so the
    // assertion is structural: exactly one new session row joined the
    // parent's.
    const p = await readPanes();
    check(
      'the left pane lists the parent plus the spawned child',
      p.leftRows.length === 2,
      p.leftRows.join(' | ')
    );
  });
});
