import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizedProgress, scrollFraction, sortedChapters } from '../src/reading-progress.js';

test('migrates old CFI-only progress and rejects corrupt positions', () => {
  assert.deepEqual(normalizedProgress({ cfi: 'epubcfi(/6/2)', read: [2, 2, -1, '1'] }), { cfi: 'epubcfi(/6/2)', index: null, scroll: null, read: [2] });
  assert.equal(normalizedProgress({ scroll: 9 }).scroll, 1);
});

test('scroll position and chapter sorting', () => {
  assert.equal(scrollFraction({ scrollTop: 300, scrollHeight: 800, clientHeight: 200 }), 0.5);
  assert.equal(scrollFraction({ scrollTop: 0, scrollHeight: 200, clientHeight: 200 }), 0);
  const chapters = [0, 1, 2].map(index => ({ index, order: index }));
  assert.deepEqual(sortedChapters(chapters, [1], 'unread').map(item => item.index), [0, 2, 1]);
  assert.deepEqual(sortedChapters(chapters, [], 'desc').map(item => item.index), [2, 1, 0]);
});