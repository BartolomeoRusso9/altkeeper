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
  '.action-row > button { flex: 0 0 184px; width: 184px;',
  'justify-content: center; text-align: center;',
  'class="row action-row"',
  'id="extension-choice"',
  'id="extension-list"',
  'id="ext-confirm"',
  'id="ext-all"',
  'id="ext-none"',
  '"/api/install/inspect"',
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
// Safari icon and already-approved Discord test stay intact through localization.
assert(html.includes('rel="apple-touch-icon"'));
assert(html.includes('rel="icon"'));
assert(html.includes('"/api/discord/test"'));
console.log("Localized dashboard JavaScript syntax, actions, labels and layout OK");
