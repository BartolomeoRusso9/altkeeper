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
  '<html lang="en">',
  'function profilesText(n)',
  'data-i18n="Your apps"',
  'data-i18n="Send test notification"',
  'data-i18n="Sign and install"',
  'data-i18n="Discord failure notifications"',
  'data-i18n="Pair iPhone over USB"',
  'data-i18n="Connection addresses"',
  'data-i18n="Save addresses"',
  'data-i18n="Anisette server"',
  'data-i18n="Test server"',
  'id="auto-pair-enabled"',
  'id="btn-cancel-wait"',
  '"/api/phone-addresses"',
  '"/api/auto-pair"',
  '"/api/pair-usb"',
  '"/api/renew/cancel-wait"',
  '"/api/anisette"',
  '"/api/anisette/test"',
  'document.hidden',
  'document.execCommand("copy")',
  'white-space: pre-wrap; overflow-wrap: anywhere;',
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
]) {
  assert(html.includes(snippet), "Missing localized/responsive UI: " + snippet);
}
assert(!/[\u1100-\u11FF\u3130-\u318F\uAC00-\uD7A3]/.test(html), "dashboard must not contain Korean text");
assert(!html.includes("language-select") && !html.includes("locale ==="), "language switcher was removed");
for (const id of [
  "btn-due", "btn-all", "btn-preview",
  "auto-enable", "auto-disable",
  "discord-save", "discord-clear", "discord-test",
  "phone-addresses-save", "btn-pair-usb", "anisette-save",
  "anisette-clear", "anisette-test", "auto-pair-enabled",
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
  vm.runInContext('let timer = null, refreshInFlight = false, refreshQueued = false, selectedDeviceId = "default";\n' +
    'function deviceHeaders() { return { "X-AltKeeper-Device": selectedDeviceId }; }\n' +
    api[0] + '\n' + refresh[0], context);
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

// A restarted server resets its sequence. Fetch its startup lines before
// advancing the cursor, and keep the old cursor if that recovery request fails.
async function checkLogPolling() {
  const view = { textContent: "before restart", scrollTop: 0, clientHeight: 100, scrollHeight: 100 };
  const requests = [], responses = [], timers = [];
  const context = vm.createContext({
    $: id => { assert.equal(id, "logs-view"); return view; },
    t: text => text,
    document: { hidden: false },
    AbortController,
    banner: (...args) => { context.lastBanner = args; },
    clearTimeout: () => {},
    api: async path => {
      requests.push(path);
      const response = responses.shift();
      if (response instanceof Error) throw response;
      assert(response, "unexpected log request");
      return response;
    },
    setTimeout: (_fn, delay) => { timers.push(delay); return timers.length; },
  });
  const poll = embedded[1].match(/async function pollLogs\(\) \{[\s\S]*?\r?\n\}/);
  assert(poll);
  vm.runInContext('let logSeq = 100, logsPollTimer = null, selectedDeviceId = "default";\n' + poll[0], context);
  responses.push({ seq: 2, lines: [] }, { seq: 3, lines: ["startup", "ready", "listening"] });
  await vm.runInContext('pollLogs()', context);
  assert.deepEqual(requests.splice(0), ["/api/logs?since=100", "/api/logs?since=0"]);
  assert.equal(view.textContent, "before restart\nstartup\nready\nlistening");
  assert.equal(vm.runInContext('logSeq', context), 3);
  assert.equal(view.scrollTop, view.scrollHeight);

  responses.push({ seq: 4, lines: ["next"] });
  await vm.runInContext('pollLogs()', context);
  assert.deepEqual(requests.splice(0), ["/api/logs?since=3"]);
  assert(view.textContent.endsWith("\nnext"));

  responses.push({ seq: 1, lines: [] }, new Error("temporarily offline"));
  await vm.runInContext('pollLogs()', context);
  assert.equal(vm.runInContext('logSeq', context), 4);
  assert.deepEqual(requests.splice(0), ["/api/logs?since=4", "/api/logs?since=0"]);
  assert.deepEqual(context.lastBanner, ["logs-message", "Could not load logs: temporarily offline", "bad"]);

  responses.push({ seq: 0, lines: [] }, { seq: 0, lines: [] });
  await vm.runInContext('pollLogs()', context);
  assert.equal(vm.runInContext('logSeq', context), 0);
  assert(view.textContent.endsWith("\nnext"));
  assert.deepEqual(timers, [10000, 2000, 10000, 2000, 10000, 2000, 10000, 2000]);
}
checkLogPolling().catch(error => { console.error(error); process.exitCode = 1; });
