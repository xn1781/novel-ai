import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createCipheriv, pbkdf2Sync, randomBytes, webcrypto } from 'node:crypto';
import { deriveKey, readCatalog } from '../src/crypto.js';

// Match the Node packer exactly without using a real book or a real passphrase.
globalThis.crypto ??= webcrypto;
test('browser key opens a catalog encrypted with the packer algorithm', async () => {
  const salt = randomBytes(16);
  const iv = randomBytes(12);
  const passphrase = 'disposable-test-passphrase';
  const books = [{ id: 'a'.repeat(32), title: 'Example novel' }];
  const nodeKey = pbkdf2Sync(passphrase, salt, 600000, 32, 'sha256');
  const cipher = createCipheriv('aes-256-gcm', nodeKey, iv);
  const data = Buffer.concat([cipher.update(JSON.stringify(books)), cipher.final(), cipher.getAuthTag()]);
  const catalog = { catalog: { iv: iv.toString('base64'), data: data.toString('base64') } };
  const key = await deriveKey(passphrase, salt.toString('base64'));
  assert.deepEqual(await readCatalog(key, catalog), books);
  const wrong = await deriveKey('wrong-disposable-passphrase', salt.toString('base64'));
  await assert.rejects(readCatalog(wrong, catalog), { name: 'OperationError' });
});