// Mock leg, feature 1 (phase 1 §4): a plain streamed text turn against the
// deterministic tau-mock-llm. The full user path is what gets exercised —
// LeftPane .new creates the session, the model menu switches it to
// mock/mock-model, the Composer sends — and the scripted lorem-ipsum turn
// is asserted entry-by-entry and in the DOM.
import path from 'node:path';
import { browser } from '@wdio/globals';
import { expect } from 'expect-webdriverio';
import {
  bootCheck,
  check,
  ensureWorkspace,
  newSessionViaUI,
  readDom,
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

// The scenario's marker (dogfood/e2e-mocks/e2e-text-turn.json) and the
// scripted reply it streams.
const MARKER = 'e2e-text-turn';
const TEXT = 'Lorem ipsum dolor sit amet, consectetur adipiscing elit';

describe('mock E2E: a streamed text turn (deterministic mock LLM)', () => {
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
        .saveScreenshot(path.join(artifacts, `failure-text-${String(failures).padStart(2, '0')}.png`))
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

  it('a Composer send streams the scripted text turn to committed entries', async () => {
    const before = (await readStore()).entries ?? 0;
    await uiSend(`${MARKER}: please respond`);
    const settled = await waitSettle(60000);
    check(
      'the turn committed a user entry and a message entry',
      settled.entries === before + 2 && settled.entryKinds.slice(-2).join(',') === 'user,message',
      `entries ${before} → ${settled.entries} (${settled.entryKinds})`
    );
    check(
      'the assistant entry is the scripted lorem text',
      (settled.lastText ?? '').includes(TEXT),
      `last="${(settled.lastText ?? '').slice(0, 48)}"`
    );
    check(
      'usage committed from response.completed matches the scenario (210 in / 64 out)',
      settled.usage?.in === 210 && settled.usage?.out === 64,
      JSON.stringify(settled.usage)
    );
  });

  it('the transcript renders the streamed text in the DOM', async () => {
    const d = await readDom();
    check(
      'the DOM transcript shows the streamed text',
      (d.lastText ?? '').includes(TEXT),
      `dom last="${(d.lastText ?? '').slice(0, 48)}"`
    );
  });
});
