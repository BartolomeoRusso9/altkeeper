"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

// This dashboard is shipped as include_str!(\"web/index.html\"), not a separate JS file.
// Compile its exact embedded script to catch JavaScript regressions that cargo test misses.
const html = fs.readFileSync(path.join(__dirname, "../src/web/index.html"), "utf8");
const embedded = html.match(/<script>([\s\S]*?)<\/script>/);
assert(embedded, "embedded dashboard JavaScript missing");
new vm.Script(embedded[1], { filename: "src/web/index.html" });

for (const snippet of [
  '<html lang="ko">',
  'id="language-select"',
  'function applyStatic()',
  'function profilesText(n)',
  'locale === "ko"',
  'data-i18n="Your apps"',
  'data-i18n="Send test notification"',
  'data-i18n="Sign and install"',
  'data-i18n="Discord failure notifications"',
  '.action-row { justify-content: flex-start; align-items: stretch; flex-wrap: nowrap; }',
  'flex: 1 1 0; min-width: 0; max-width: 184px;',
  '.action-row { gap: 6px; }',
  'word-break: keep-all; overflow-wrap: anywhere;',
  'justify-content: center; text-align: center;',
  'class="row action-row"',
  'id="extension-choice"',
  'id="extension-list"',
  'id="ext-confirm"',
  'id="ext-all"',
  'id="ext-none"',
  '"/api/install/inspect?customize="',
  'id="install-display-name"',
  'id="install-bundle-id"',
  'data-i18n="New app name (optional)"',
  'data-i18n="New Bundle ID (optional)"',
  '"/api/install/confirm"',
  'function showExtensionOptions(info)',
  'function updateExtensionSummary()',
  'const KO =',
]) {
  assert(html.includes(snippet), "Missing localized/responsive UI: " + snippet);
}
for (const id of [
  "btn-due", "btn-all", "btn-preview",
  "auto-enable", "auto-disable",
  "discord-save", "discord-clear", "discord-test",
  "btn-install", "btn-login", "btn-code",
]) {
  assert(html.includes('id="' + id + '"'), "Missing control: " + id);
}
// Under 420px, action groups must stay on one row and divide the available
// card width into three compact button slots without overflowing.
const iphoneMiniContent = 375 - 2 * 16 - 2 * 16 - 2;
assert((iphoneMiniContent - 2 * 6) / 3 > 90, "iPhone mini action buttons should fit 3 across");
assert(html.includes('#code-form .row { justify-content: flex-start; flex-wrap: nowrap; gap: 8px; }'));
assert(html.includes('<div class="row" style="margin-top:10px">'));

// Safari icon and already-approved Discord test stay intact through localization.
assert(html.includes('rel="apple-touch-icon"'));
assert(html.includes('rel="icon"'));
assert(html.includes('"/api/discord/test"'));
console.log("Localized dashboard JavaScript syntax, actions, labels and layout OK");

// HTTPS is an additive, private LAN proxy. Do not expose a public wildcard
// port or change the existing AltKeeper/AltServer service and its credentials.
{
  const httpsDir = path.join(__dirname, "../examples/local-https");
  const compose = fs.readFileSync(path.join(httpsDir, "compose.yml"), "utf8");
  const caddy = fs.readFileSync(path.join(httpsDir, "Caddyfile"), "utf8");
  assert(compose.includes("192.168.0.62}:8788:8788"), "proxy must bind only the LAN IP");
  assert(compose.includes("host.docker.internal:host-gateway"));
  assert(compose.includes("./caddy-data:/data"), "CA must persist across restarts");
  assert(!compose.includes("network_mode: host"), "proxy must not take over host networking");
  assert(caddy.includes("tls internal"), "never use public ACME for LAN HTTPS");
  assert(caddy.includes("reverse_proxy host.docker.internal:8787"));
  assert(caddy.includes("auto_https disable_redirects"), "preserve HTTP on :8787");
}

// Wake Lock is a browser-only opt-in/test feature and automatic only for installs.
// Never resurrect the abandoned silent video/floating card.
async function checkWakeLock() {
  assert(html.includes('id="btn-wake-test"'));
  assert(html.includes('id="wake-status"'));
  assert(!html.includes('id="keepawake-test-card"'));
  assert(!html.includes('id="install-keepawake-video"'));
  assert(!html.includes("data:video/mp4;base64,"));
  const source = embedded[1];
  const start = source.indexOf("const WAKE_TEST_MS = ");
  const end = source.indexOf("function days(n) {", start);
  assert(start >= 0 && end > start, "wake helper source not found");
  const nodes = {
    "btn-wake-test": { disabled: false, textContent: "", onclick: null },
    "wake-status": { textContent: "" },
  };
  const listeners = {};
  let hidden = false, now = 50000, releaseCount = 0, requestCount = 0;
  let nextTimer = 1;
  const intervals = new Map();
  const windowMock = {
    isSecureContext: false,
    addEventListener(type, callback) { listeners[type] = callback; },
  };
  const navigatorMock = {
    wakeLock: {
      request(type) {
        assert.equal(type, "screen");
        requestCount++;
        const sentinel = {
          released: false, onRelease: null,
          addEventListener(type, fn) { assert.equal(type, "release"); this.onRelease = fn; },
          release() {
            if (!this.released) { this.released = true; releaseCount++; this.onRelease?.(); }
            return Promise.resolve();
          },
        };
        return Promise.resolve(sentinel);
      },
    },
  };
  const ctx = vm.createContext({
    document: {
      get hidden() { return hidden; },
      getElementById(id) { return nodes[id]; },
      addEventListener(type, callback) { listeners[type] = callback; },
    },
    window: windowMock, navigator: navigatorMock,
    Date: { now: () => now },
    setInterval(fn, delay) {
      assert.equal(delay, 1000);
      const key = nextTimer++;
      intervals.set(key, fn);
      return key;
    },
    clearInterval(key) { intervals.delete(key); },
  });
  vm.runInContext('const $ = id => document.getElementById(id);\n'
    + 'function t(x) {return x;}\nlet state = null;\n'
    + source.slice(start, end), ctx);
  const call = cmd => vm.runInContext(cmd, ctx);
  const settle = async () => { await Promise.resolve(); await Promise.resolve(); };
  // In HTTP / other nonsecure contexts never issue a wake request.
  call("syncWakeLock()");
  assert.equal(requestCount, 0);
  assert.equal(nodes["btn-wake-test"].disabled, true);
  assert(nodes["wake-status"].textContent.includes("HTTPS"));

  // Manual five-minute test is client-only and is not an app install.
  windowMock.isSecureContext = true;
  nodes["btn-wake-test"].onclick();
  await settle();
  assert.equal(requestCount, 1);
  assert.equal(intervals.size, 1);
  assert(nodes["wake-status"].textContent.includes("Wake lock active"));
  nodes["btn-wake-test"].onclick();
  await settle();
  assert.equal(intervals.size, 0);
  assert.equal(releaseCount, 1);
  call('state = {job:{kind:"renew",running:true}}; syncWakeLock()');
  assert.equal(requestCount, 1, "renewals must never hold a wake lock");

  // Installation acquires automatically, then releases on completion/failure.
  call('state = {job:{kind:"install",running:true}}; syncWakeLock()');
  await settle();
  assert.equal(requestCount, 2);
  assert.equal(nodes["btn-wake-test"].disabled, true);
  call('state = {job:{kind:"install",running:false,ok:false}}; syncWakeLock()');
  await settle();
  assert.equal(releaseCount, 2);

  // Backgrounding an in-progress install releases the lock.
  call('state = {job:{kind:"install",running:true}}; syncWakeLock()');
  await settle();
  assert.equal(requestCount, 3);
  hidden = true;
  listeners.visibilitychange();
  await settle();
  assert.equal(releaseCount, 3);
  hidden = false;
  listeners.visibilitychange();
  await settle();
  assert.equal(requestCount, 4);
  call('state = {job:{kind:"renew",running:true}}; syncWakeLock()');
  await settle();

  // Expiration uses real elapsed time, not timer count, and does not
  // interfere with server jobs.
  call('state = {job:{kind:"",running:false}}');
  nodes["btn-wake-test"].onclick();
  await settle();
  assert.equal(intervals.size, 1);
  now += 300001;
  for (const tick of [...intervals.values()]) tick();
  await settle();
  assert.equal(intervals.size, 0);
  assert(nodes["wake-status"].textContent === "");
}
checkWakeLock().catch(error => { console.error(error); process.exitCode = 1; });

// Installation progress stays next to the IPA controls, including its final
// failure/success. A later renewal moves the same panel back without copying
// log nodes or losing the reader's scroll position.
{
  const nodes = { "app-job-slot": {}, "install-job-slot": {}, "job": { scrollTop: 42 } };
  for (const id of ["app-job-slot", "install-job-slot"]) {
    nodes[id].append = panel => { panel.parentElement = nodes[id]; };
  }
  nodes.job.parentElement = nodes["app-job-slot"];
  const context = vm.createContext({ $: id => nodes[id], t: x => x, uploading: false });
  for (const name of ["placeJob", "installHint"]) {
    const fn = embedded[1].match(new RegExp("function " + name + "\\([^)]*\\) \\{[\\s\\S]*?\\r?\\n\\}"));
    assert(fn, "missing installation UI helper: " + name);
    vm.runInContext(fn[0], context);
  }
  vm.runInContext('placeJob({kind:"install",running:true})', context);
  assert.equal(nodes.job.parentElement, nodes["install-job-slot"]);
  vm.runInContext('placeJob({kind:"install",running:false,ok:false})', context);
  assert.equal(nodes.job.parentElement, nodes["install-job-slot"]);
  assert.equal(nodes.job.scrollTop, 42);
  vm.runInContext('placeJob({kind:"renew",running:true})', context);
  assert.equal(nodes.job.parentElement, nodes["app-job-slot"]);
  const hint = vm.runInContext('installHint({busy:true,job:{kind:"install",running:true}})', context);
  assert(hint.includes("Signing and installing"));
  assert.equal(vm.runInContext('installHint({busy:true,job:{kind:"install",running:false}})', context), "Checking…");
}

// Exercise the actual dashboard functions with controlled responses. An edit
// must survive polling, and a snapshot started before an action must not paint
// over the newer state once another refresh has been requested.
async function checkDashboardState() {
  const controls = { "auto-time": { value: "" }, "auto-days": { value: "" } };
  for (const control of Object.values(controls)) {
    control.listeners = {};
    control.addEventListener = (event, fn) => { control.listeners[event] = fn; };
  }
  const pending = [], rendered = [], timers = new Map();
  let nextTimer = 0;
  const context = vm.createContext({
    document: { getElementById: id => controls[id] },
    fetch: (_path, options) => new Promise((resolve, reject) => {
      pending.push({ resolve, reject, signal: options.signal });
      options.signal?.addEventListener("abort", () => reject(new Error("aborted")));
    }),
    AbortController,
    setTimeout: (fn, delay) => { timers.set(++nextTimer, { fn, delay }); return nextTimer; },
    clearTimeout: id => timers.delete(id),
    render: s => rendered.push(s.version),
    banner() {}, t: x => x,
  });
  const source = embedded[1];
  // Run only the draft declaration/helper/listeners: the full browser render
  // needs many unrelated DOM controls.
  const helper = source.match(/function syncAutoRenewInputs\(auto\) \{[\s\S]*?\r?\n\}\r?\nfor \(const id of \["auto-time", "auto-days"\]\) \{[\s\S]*?\r?\n\}/);
  assert(helper);
  vm.runInContext('const $ = id => document.getElementById(id); let autoDraftDirty = false, autoSavePending = false;\n' + helper[0], context);
  vm.runInContext('syncAutoRenewInputs({time:"21:17",min_days:3})', context);
  controls["auto-time"].value = "22:30";
  controls["auto-time"].listeners.input();
  controls["auto-days"].value = "5";
  controls["auto-days"].listeners.change();
  vm.runInContext('syncAutoRenewInputs({time:"21:17",min_days:3})', context);
  assert.equal(controls["auto-time"].value, "22:30");
  assert.equal(controls["auto-days"].value, "5");
  vm.runInContext('autoSavePending = true; syncAutoRenewInputs({time:"21:17",min_days:3})', context);
  assert.equal(controls["auto-time"].value, "22:30", "an old response after save must not reset the draft");
  vm.runInContext('syncAutoRenewInputs({time:"22:30",min_days:5})', context);
  assert.equal(controls["auto-time"].value, "22:30");
  assert.equal(vm.runInContext('autoDraftDirty', context), false, "server confirmation clears the draft");

  const api = source.match(/async function api\(path, body, signal\) \{[\s\S]*?\r?\n\}/);
  const refresh = source.match(/async function refresh\(\) \{[\s\S]*?\r?\n\}/);
  assert(api && refresh);
  vm.runInContext('let timer = null, refreshInFlight = false, refreshQueued = false;\n' + api[0] + '\n' + refresh[0], context);
  const snapshot = version => ({ version, busy:false, phone:{checking:false}, login:{phase:""}, job:{running:false} });
  const reply = (request, version) => request.resolve({ ok:true, json:async () => snapshot(version) });
  const first = vm.runInContext('refresh()', context);
  await vm.runInContext('refresh()', context);
  assert.equal(pending.length, 1, "concurrent refresh should share one active request");
  reply(pending[0], "older");
  await first;
  assert.deepEqual(rendered, [], "queued refresh must suppress the stale response");
  const queued = [...timers.values()].find(t => t.delay === 0);
  assert(queued, "queued refresh should start immediately");
  queued.fn();
  assert.equal(pending.length, 2);
  reply(pending[1], "newer");
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(rendered, ["newer"]);
  const nextPoll = [...timers.values()].find(t => t.delay === 15000);
  assert(nextPoll, "polling should continue after a response");
  nextPoll.fn();
  assert.equal(pending.length, 3);
  const timeout = [...timers.values()].find(t => t.delay === 10000);
  assert(timeout, "status requests need a timeout");
  timeout.fn();
  await new Promise(resolve => setImmediate(resolve));
  assert(pending[2].signal.aborted);
  assert([...timers.values()].some(t => t.delay === 15000), "polling should recover after timeout");
}
checkDashboardState().catch(error => { console.error(error); process.exitCode = 1; });

