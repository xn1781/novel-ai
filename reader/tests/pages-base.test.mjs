import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pagesBase } from '../src/pages-base.js';

test('GitHub user Pages repos serve at the domain root', () => {
  assert.equal(pagesBase('xn1781/xn1781.github.io'), '/');
  assert.equal(pagesBase('Owner/owner.github.io'), '/');
});

test('project Pages repos serve under their repository path', () => {
  assert.equal(pagesBase('owner/novel-ai'), '/novel-ai/');
  assert.equal(pagesBase(undefined), '/');
  assert.throws(() => pagesBase('invalid'), /owner\/repository/);
});