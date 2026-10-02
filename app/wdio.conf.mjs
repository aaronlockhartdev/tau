// WebdriverIO config for the real-app E2E (roadmap G/G2): the DEBUG Tauri
// binary against a test workspace, driven over the embedded WebDriver
// server (tauri-plugin-wdio-webdriver). Replaces the retired tauri-pilot
// harness. Two legs (the replay leg retired in #50 — its dogfood pair's
// coverage moved to the mock leg's subagent scenario and the
// compaction-bearing stress fixture):
//   stress — the generated 10k-entry fixture with 50 compaction records
//     (windowing, boot pin under load, the 500 ms perf bar): the scale
//     leg, run in CI and locally.
//   mock — the deterministic tau-mock-llm scenarios (hash-pinned): streamed
//     text, tool-call rendering, multi-turn sequencing, and a scripted
//     subagent spawn against the fake LLM.
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const APP = import.meta.dirname;
const ROOT = path.join(APP, '..');
const FIXTURE = path.join(ROOT, 'target', 'test-fixture', 'session.jsonl');
// Re-pinned 2026-10-02 (#49): the fixture gained 50 compaction records
// (one per goal), one of them oversized — the session file's hash moved,
// and the zstd sidecar now ships beside it (target/test-fixture/blobs/).
const FIXTURE_SHA256 = '09796263a8795b40a8663537f387283cad8fb9cf883fb0eeec8b4f354a1e9b74';
const FIXTURE_SESSION = 'session';
// The deterministic mock LLM (phase 1 §4): hash-pinned scenario files
// served by the tau-mock-llm binary; the mock leg's workspace points its
// `mock` provider at it and the specs switch their sessions to mock-model.
const MOCK_PORT = 8123;
const MOCK_SCENARIOS = {
  'e2e-text-turn.json': '5c1643e3bf0086324282fc0f605d12f7b8b7fabdec9e0556309ca13df7ebb9a9',
  'e2e-tool-turn.json': '6aca5c25e849fbb2419c525251fd6987554c6ca8248e7dcd4f5627b8683b8479',
  'e2e-multi-turn.json': '020a91b0c2e0f7ae80fa1e8d3f265bfe6f5ca66b4d41823ef2fc255eedeeb115',
  'e2e-subagent-parent.json': '9f1a0758f6a50aab5e2915cec8040e884d9157bbcb215c78364e44adde69612e',
  'e2e-subagent-child.json': '2f139ff0766deecc40b6d13b9182e91ff0f3ad7fd6a848bc3aa443670b49571e'
};
const VITE_URL = 'http://127.0.0.1:5173/';
const OUTPUT_DIR = path.join(ROOT, 'target', 'e2e');

// No TAU_E2E_MODE = both legs, locally and in CI (#50: CI now runs the
// full set — the stress leg's budgets are met on the CI runners the way
// the mock leg's are, so the old replay-only CI split is gone).
const ALL_LEGS = ['stress', 'mock'];
const mode = process.env.TAU_E2E_MODE;
if (mode && !ALL_LEGS.includes(mode))
  throw new Error(`TAU_E2E_MODE must be one of ${ALL_LEGS.join('|')} (got ${mode})`);
const legs = mode ? [mode] : ALL_LEGS;

// One spec file per leg, except mock: one spec per scripted feature
// (text streaming, tool-call rendering, multi-turn sequencing, subagent
// spawn).
const LEG_SPECS = {
  stress: ['stress.spec.mjs'],
  mock: [
    'mock-text.spec.mjs',
    'mock-tool.spec.mjs',
    'mock-multi.spec.mjs',
    'mock-subagent.spec.mjs'
  ]
};
// The isolated HOME keeps the run from touching the real ~/.config/tau; an
// empty system dir is what makes the boot check (no workspace auto-opens)
// deterministic. It is a FIXED path (not mkdtemp) set in the test:frontend
// script's environment, because the app is spawned by the wdio CLI process —
// which never runs this config — so only the CLI's inherited env can point
// the app (and the spec's os.homedir()) at the same home.
const E2E_HOME = '/tmp/tau-e2e-home';
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'tau-e2e-'));
// The xdg dir lives under a short /tmp path (macOS SUN_PATH = 104): the
// debug binary's pilot socket name must fit the kernel's Unix path limit,
// and os.tmpdir() on macOS (/var/folders/…) would push it past it.
const xdg = path.join('/tmp', path.basename(tmp), 'xdg');
function sh(cmd, args, opts = {}) {
  const p = spawnSync(cmd, args, { cwd: ROOT, encoding: 'utf8', ...opts });
  if (p.status !== 0) throw new Error(`${cmd} ${args.join(' ')} exited ${p.status}\n${p.stderr}`);
  return p.stdout;
}

function buildFixture() {
  // A stale target/ artifact (an older generator) is the common local
  // case: regenerate when the hash, not just the absence, says so, then
  // re-check — a mismatch after regeneration is a source/pin desync.
  const sha = () => createHash('sha256').update(fs.readFileSync(FIXTURE)).digest('hex');
  if (!fs.existsSync(FIXTURE) || sha() !== FIXTURE_SHA256)
    sh('cargo', ['test', '-p', 'tau-core', '--test', 'fixture_gen']);
  if (sha() !== FIXTURE_SHA256)
    throw new Error(`fixture hash drifted after regeneration: ${sha()} != ${FIXTURE_SHA256}`);
}


function checkMockScenarios() {
  for (const [file, pinned] of Object.entries(MOCK_SCENARIOS)) {
    const sha = createHash('sha256').update(fs.readFileSync(path.join(ROOT, 'fixtures', 'e2e-mocks', file))).digest('hex');
    if (sha !== pinned) throw new Error(`mock scenario drifted: ${file}`);
  }
}

// The minimal test workspace: the session file(s) under .tau/sessions/ and
// the canned:// text provider in the *project* config (.tau/config.toml) —
// the production layering merge a session reads through (spec §12); the
// isolated HOME's system layer stays empty. One workspace PER LEG, always
// (ticket #48): the core pins a session to the *first* provider of the
// merged config (BTreeMap order) at registration and session_set_model
// only swaps the model id, and the mock leg's project config must never
// cohabit a workspace whose sessions the canned leg drives — a shared
// dir let the mock leg's config write clobber the shared project layer
// and pin the stress session to the mock provider. The stress spec reads
// TAU_E2E_WS_STRESS; the mock specs read TAU_E2E_WS_MOCK.
const CANNED_CONFIG = ['[providers.canned]', 'base_url = "canned://text"', '', '[providers.canned.models."canned-model"]', ''].join('\n');
function makeWorkspaces() {
  const dirs = {};
  // The model menu lists the *system* layer only (harness ProviderList),
  // so the mock leg also puts the mock provider in the isolated home's
  // system config: the menu then offers it, and the same-named project
  // entry keeps the merged config coherent (spec §12 layering — a
  // project entry of the same name replaces the system one).
  if (legs.includes('mock')) {
    fs.writeFileSync(
      path.join(E2E_HOME, '.config', 'tau', 'config.toml'),
      [
        '[providers.mock]',
        `base_url = "http://127.0.0.1:${MOCK_PORT}/v1"`,
        '',
        '[providers.mock.models."mock-model"]',
        '',
        '[providers.mock.models."mock-model-2"]',
        ''
      ].join('\n')
    );
  }
  // One dir per leg, in every mode (#48): 'all' used to share one dir
  // between stress + mock, and the mock leg's config write (last
  // in the loop) clobbered the shared project layer with the mock
  // provider, pinning every shared-ws session to mock.
  for (const leg of legs) {
    const ws = path.join(tmp, `ws-${leg}`);
    fs.mkdirSync(path.join(ws, '.tau', 'sessions'), { recursive: true });
    if (leg === 'stress') {
      fs.copyFileSync(FIXTURE, path.join(ws, '.tau', 'sessions', `${FIXTURE_SESSION}.jsonl`));
      // The oversized compaction record's zstd sidecar sits under the
      // store root (.tau/blobs/), beside the session file (#49).
      const blobs = path.join(ROOT, 'target', 'test-fixture', 'blobs');
      if (fs.existsSync(blobs)) fs.cpSync(blobs, path.join(ws, '.tau', 'blobs'), { recursive: true });
      fs.writeFileSync(path.join(ws, '.tau', 'config.toml'), CANNED_CONFIG);
    } else {
      // The mock provider is the sole entry: a fresh session defaults to
      // mock-model, and the twin model entries let a spec switch models
      // through the real menu UI (the mock server ignores the model id —
      // the scenario's match string selects the script).
      fs.writeFileSync(
        path.join(ws, '.tau', 'config.toml'),
        [
          '[providers.mock]',
          `base_url = "http://127.0.0.1:${MOCK_PORT}/v1"`,
          '',
          '[providers.mock.models."mock-model"]',
          '',
          '[providers.mock.models."mock-model-2"]',
          ''
        ].join('\n')
      );
    }
    dirs[leg] = ws;
  }
  fs.mkdirSync(xdg, { recursive: true, mode: 0o700 });
  return dirs;
}

function binaryPath() {
  const bin = path.join(ROOT, 'target', 'debug', 'tau-app');
  if (!fs.existsSync(bin)) throw new Error('debug binary missing after cargo build');
  return bin;
}

// The app's env as seen by the service's spawn: the isolated HOME/xdg above,
// minus DISPLAY on macOS (no X there; on Linux the xvfb-run DISPLAY is the
// webview's display and must be inherited).
function appEnv() {
  const env = { ...process.env };
  if (process.platform === 'darwin') delete env.DISPLAY;
  delete env.TAURI_PILOT_SOCKET;
  env.HOME = E2E_HOME;
  env.XDG_RUNTIME_DIR = xdg;
  return env;
}

// The debug build loads the devUrl, not the embedded dist, so the dev
// server must be up before the service spawns the app (vite's default port
// 5173 is the devUrl). Spawned directly (not through npm) so the kill
// reaches the server itself.
let devProc = null;
function startVite() {
  if (!fs.existsSync(path.join(APP, 'node_modules'))) sh('npm', ['ci'], { cwd: APP, stdio: 'inherit' });
  devProc = spawn(process.execPath, [path.join(APP, 'node_modules', 'vite', 'bin', 'vite.js'), '--host', '--strictPort'], {
    cwd: APP,
    stdio: ['ignore', 'pipe', 'pipe']
  });
  const log = (d) => process.stderr.write(`[vite] ${d}`);
  devProc.stdout.on('data', log);
  devProc.stderr.on('data', log);
}

// The deterministic mock LLM for the mock leg: the compiled binary over
// the hash-pinned scenario dir (built here so a fresh checkout works).
let mockProc = null;
function startMock() {
  sh('cargo', ['build', '-p', 'tau-mock-llm']);
  mockProc = spawn(
    path.join(ROOT, 'target', 'debug', 'tau-mock-llm'),
    ['--port', String(MOCK_PORT), '--scenarios', path.join(ROOT, 'fixtures', 'e2e-mocks')],
    { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] }
  );
  const log = (d) => process.stderr.write(`[mock-llm] ${d}`);
  mockProc.stdout.on('data', log);
  mockProc.stderr.on('data', log);
}

async function poll(fn, ms, label) {
  const t0 = Date.now();
  for (;;) {
    try {
      return await fn();
    } catch {
      if (Date.now() - t0 > ms) throw new Error(`${label} never became ready (${ms} ms)`);
      await sleep(250);
    }
  }
}

function teardown() {
  try {
    if (devProc) devProc.kill('SIGKILL');
  } catch {
    // already gone
  }
  try {
    if (mockProc) mockProc.kill('SIGKILL');
  } catch {
    // already gone
  }
}
process.on('exit', teardown);

async function onPrepare() {
  try {
    await onPrepareInner();
  } catch (err) {
    // An unlogged hook rejection surfaces as "all specs fail with env
    // unset" and loses the real reason — it must reach the log.
    console.error(`e2e: onPrepare failed: ${err.stack ?? err}`);
    throw err;
  }
}

async function onPrepareInner() {
  // Reset the isolated home so the boot check (no workspace auto-opens) and
  // the registry-persistence check start from a known-empty registry.
  fs.rmSync(E2E_HOME, { recursive: true, force: true });
  fs.mkdirSync(path.join(E2E_HOME, '.config', 'tau'), { recursive: true });
  console.log(`e2e: mode=${mode ?? 'all'}, temp workspace under ${tmp}`);
  fs.rmSync(OUTPUT_DIR, { recursive: true, force: true });
  fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  for (const leg of legs) {
    if (leg === 'stress') buildFixture();
    else checkMockScenarios();
  }
  const workspaces = makeWorkspaces();
  // The spec runs in a separate worker process — the shared context travels
  // over the worker's inherited environment (the service talks to the app
  // the same way).
  // Assigning undefined to process.env writes the string "undefined" —
  // the worker re-imports this config and would reject it.
  if (mode) process.env.TAU_E2E_MODE = mode;
  process.env.TAU_E2E_WS_MOCK = workspaces.mock ?? '';
  process.env.TAU_E2E_WS_STRESS = workspaces.stress ?? '';
  process.env.TAU_E2E_TMP = tmp;
  process.env.TAU_E2E_FIXTURE_SESSION = FIXTURE_SESSION;
  process.env.TAU_E2E_ARTIFACTS = OUTPUT_DIR;
  // The debug build loads the devUrl on every platform (Linux included —
  // xvfb gives the webview its display, vite its page).
  console.log('e2e: starting the vite dev server on the devUrl');
  startVite();
  await poll(
    () => fetch(VITE_URL).then((r) => {
      if (!r.ok) throw new Error(`vite answered ${r.status}`);
    }),
    60000,
    'vite dev server'
  );
  if (legs.includes('mock')) {
    startMock();
    await poll(
      () =>
        fetch(`http://127.0.0.1:${MOCK_PORT}/models`).then((r) => {
          if (!r.ok) throw new Error(`mock answered ${r.status}`);
        }),
      30000,
      'the mock LLM'
    );
  }
}

function onComplete() {
  teardown();
}

export const config = {
  specs: legs.flatMap((leg) => LEG_SPECS[leg].map((f) => path.join(APP, 'tests', 'e2e', f))),

  maxInstances: 1,

  // Framework: mocha (one framework, minimal config — the WDIO v9 runner
  // framework in this registry; the vitest framework package is not
  // published for v9 here).
  framework: 'mocha',
  mochaOpts: {
    ui: 'bdd',
    timeout: 300000
  },

  services: [
    ['@wdio/tauri-service', {
      // The debug binary (the service spawns it; the Vite dev server is
      // started in onPrepare above, not by the service).
      appBinaryPath: binaryPath(),
      env: appEnv(),
      // The embedded provider is the default on every platform; it needs
      // tauri-plugin-wdio-webdriver registered in the app (debug builds).
      driverProvider: 'embedded',
      // The embedded server takes longer to come up on a starved runner.
      startTimeout: 90000,
      commandTimeout: 60000,
      // Failure artifacts: the app's captured stdout/stderr land in
      // OUTPUT_DIR (the CI jobs upload it on failure).
      captureBackendLogs: true,
      captureFrontendLogs: true,
      backendLogLevel: 'debug',
      frontendLogLevel: 'debug'
    }]
  ],

  capabilities: [{
    browserName: 'tauri',
    'tauri:options': {
      // Resolved relative to the repo root, per the roadmap G2 shape.
      application: path.join(ROOT, 'target', 'debug', 'tau-app')
    }
  }],

  outputDir: OUTPUT_DIR,
  logLevel: 'info',
  bail: 0,
  // The stack's own CI flake log (F1, macOS embedded-provider idle stalls)
  // clears on retry: one same-run recovery per spec file on CI; local stays
  // 0 so a flake that clears on retry is still visible where it matters.
  specFileRetries: process.env.CI ? 1 : 0,

  // Framework-level waiting (the structural fix for the pilot era's fixed
  // 10 s eval budget): generous element-wait and connection-retry budgets;
  // the spec's state convergence uses expect-webdriverio auto-wait with
  // 120 s-class deadlines on the big reloads.
  waitforTimeout: 30000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  // A single W3C command (executeScript etc.) is bounded: a wedged read
  // throws instead of eating the whole convergence deadline, and the
  // auto-wait retries it.
  timeout: 60000,

  reporters: ['spec'],

  onPrepare,
  onComplete
};
