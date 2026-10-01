// Mock leg, feature 3 (phase 1 §4): session creation plus a scripted
// multi-turn sequence. The scenario (test/fixtures/e2e-mocks/e2e-multi-turn.json)
// answers its Nth request with its Nth scripted reply — three Composer
// sends walk the sequence 1 → 2 → 3, each pair (user + assistant)
// committed in order.
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  ensureWorkspace,
  newSessionViaUI,
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

// The scenario's marker; each send carries it so every request re-selects
// the same scenario (the server's turn counter advances per request).
const MARKER = 'e2e-multi-turn';

describe('mock E2E: session creation + a scripted 3-turn sequence', () => {
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
        .saveScreenshot(path.join(artifacts, `failure-multi-${String(failures).padStart(2, '0')}.png`))
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

  it('three Composer sends walk the scripted 1 → 2 → 3 sequence', async () => {
    for (let i = 1; i <= 3; i++) {
      const before = (await readStore()).entries ?? 0;
      await uiSend(`${MARKER} round ${i}`);
      const s = await waitSettle(60000);
      check(
        `turn ${i}: committed a user + assistant pair`,
        s.entries === before + 2,
        `entries ${before} → ${s.entries}`
      );
      check(
        `turn ${i}: the scripted reply "${i} of 3" arrived`,
        (s.lastText ?? '').includes(`multi-turn ${i} of 3`),
        `last="${(s.lastText ?? '').slice(0, 48)}"`
      );
    }
  });

  it('the session holds all six entries in send order', async () => {
    const s = await readStore();
    check(
      'seven entries: the model note + three user / three message, in order',
      s.entries === 7 && s.entryKinds.slice(-6).join(',') === 'user,message,user,message,user,message',
      JSON.stringify(s.entryKinds)
    );
  });
});
