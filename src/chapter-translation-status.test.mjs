import test from "node:test";
import assert from "node:assert/strict";
import { translationStatus, sourceLanguageTitle, chapterOptionLabel } from "./chapter-translation-status.js";

test("flags any Korean or Japanese script in saved output, not source", () => {
  const source = "안녕하세요 오늘은 새로운 이야기가 시작됩니다.";
  assert.equal(translationStatus({ text: source, translated: null }, "Korean"), "No saved translation");
  assert.equal(translationStatus({ text: source, translated: source }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: "안녕하세요", translated: "안녕하세요" }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "Hello, a new story starts today." }, "Korean"), "No Korean/Japanese detected");
  assert.equal(translationStatus({ text: source, translated: "안녕하세요\nHello, a new story starts today." }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "A whole chapter in English.\n\n그녀는 문을 열고 복도로 나갔다. 갑자기 바람이 불어왔다.\n\nThe end." }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "민수 said hello to 지은, then left the room and went home." }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "Hello there. ㅋ" }, "Korean"), "Contains Korean/Japanese (saved text)");
  assert.equal(chapterOptionLabel({ title: "Chapter 27", text: source, translated: source }, 26, "Korean"), "27. Chapter 27 — Contains Korean/Japanese (saved text)");
  assert.equal(chapterOptionLabel({ title: "제27화", text: source, translated: null }, 26, "Korean"), "27. English title not saved — No saved translation");
  assert.equal(chapterOptionLabel({ title: "Chapter 27", text: source, translated: "English translation" }, 26, "Korean"), "27. Chapter 27 — No Korean/Japanese detected");
  assert.equal(translationStatus({ text: source, translated: source }, "Japanese"), "Contains Korean/Japanese (saved text)");
});

test("recognizes Japanese kana, kanji, and source-language titles", () => {
  const source = "これは新しい物語の始まりです。";
  assert.equal(translationStatus({ text: source, translated: source }, "Japanese"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "これは\nThis is the start of a new story." }, "Japanese"), "Contains Korean/Japanese (saved text)");
  assert.equal(translationStatus({ text: source, translated: "The 大 tree" }, "Japanese"), "Contains Korean/Japanese (saved text)");
  assert.equal(sourceLanguageTitle("제10화", "Korean"), true);
  assert.equal(sourceLanguageTitle("Chapter 10", "Korean"), false);
  assert.equal(sourceLanguageTitle("第一話", "Japanese"), true);
});