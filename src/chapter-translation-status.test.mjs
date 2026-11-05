import test from "node:test";
import assert from "node:assert/strict";
import { translationStatus, sourceLanguageTitle } from "./chapter-translation-status.js";

test("distinguishes missing, source-language and saved English text", () => {
  const source = "안녕하세요 오늘은 새로운 이야기가 시작됩니다.";
  assert.equal(translationStatus({ text: source, translated: null }, "Korean"), "No saved translation");
  assert.equal(translationStatus({ text: source, translated: source }, "Korean"), "Likely untranslated (saved text)");
  assert.equal(translationStatus({ text: "안녕하세요", translated: "안녕하세요" }, "Korean"), "Likely untranslated (saved text)");
  assert.equal(translationStatus({ text: source, translated: "Hello, a new story starts today." }, "Korean"), "Saved translation");
  assert.equal(translationStatus({ text: source, translated: "안녕하세요\nHello, a new story starts today." }, "Korean"), "Saved translation");
});

test("recognizes Japanese kana but does not classify a few names as untranslated", () => {
  const source = "これは新しい物語の始まりです。";
  assert.equal(translationStatus({ text: source, translated: source }, "Japanese"), "Likely untranslated (saved text)");
  assert.equal(translationStatus({ text: source, translated: "これは\nThis is the start of a new story." }, "Japanese"), "Saved translation");
  assert.equal(sourceLanguageTitle("제10화", "Korean"), true);
  assert.equal(sourceLanguageTitle("Chapter 10", "Korean"), false);
  assert.equal(sourceLanguageTitle("第一話", "Japanese"), true);
});