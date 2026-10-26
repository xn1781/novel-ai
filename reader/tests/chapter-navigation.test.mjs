import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adjacentSection } from '../src/chapter-navigation.js';

test('chapter navigation follows linear spine entries and skips non-linear entries', () => {
  const first = { href: 'first.xhtml', next: () => last, prev: () => undefined };
  const extra = { href: 'notes.xhtml', next: () => undefined, prev: () => undefined };
  const last = { href: 'last.xhtml', next: () => undefined, prev: () => first };
  const spine = { get: index => [first, extra, last][index] || null };
  assert.equal(adjacentSection(spine, 0, 'next'), last);
  assert.equal(adjacentSection(spine, 2, 'prev'), first);
  assert.equal(adjacentSection(spine, 0, 'prev'), null);
  assert.equal(adjacentSection(spine, 2, 'next'), null);
  assert.equal(adjacentSection(spine, null, 'next'), null);
});