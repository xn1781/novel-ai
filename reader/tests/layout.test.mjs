import { test } from 'node:test';
import assert from 'node:assert/strict';
import { layoutFor } from '../src/layout.js';

test('continuous mode uses the continuous EPUB manager; other modes show one section', () => {
  assert.deepEqual(layoutFor('scrolled-continuous'), { manager: 'continuous', flow: 'scrolled-continuous' });
  assert.deepEqual(layoutFor('scrolled-chapter'), { manager: 'default', flow: 'scrolled-doc' });
  assert.deepEqual(layoutFor('paginated'), { manager: 'default', flow: 'paginated' });
});