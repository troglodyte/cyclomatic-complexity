const { test } = require("node:test");
const assert = require("node:assert/strict");
const { documentSelector } = require("../src/selector.js");

const LANGUAGES = ["javascript", "javascriptreact", "typescript", "typescriptreact", "rust"];

test("saved files of every language are scored", () => {
  for (const language of LANGUAGES) {
    assert.ok(
      documentSelector().some((f) => f.scheme === "file" && f.language === language),
      language,
    );
  }
});

test("unsaved (untitled) buffers of every language are scored too", () => {
  for (const language of LANGUAGES) {
    assert.ok(
      documentSelector().some((f) => f.scheme === "untitled" && f.language === language),
      language,
    );
  }
});

test("no other scheme or language is claimed", () => {
  assert.equal(documentSelector().length, LANGUAGES.length * 2);
});
