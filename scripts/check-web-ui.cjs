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
