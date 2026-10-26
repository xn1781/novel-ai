const encoder = new TextEncoder();
const decoder = new TextDecoder();
const decode64 = value => Uint8Array.from(atob(value), char => char.charCodeAt(0));

export async function deriveKey(passphrase, salt) {
  const material = await crypto.subtle.importKey('raw', encoder.encode(passphrase), 'PBKDF2', false, ['deriveKey']);
  return crypto.subtle.deriveKey(
    { name: 'PBKDF2', salt: decode64(salt), iterations: 600000, hash: 'SHA-256' },
    material, { name: 'AES-GCM', length: 256 }, false, ['decrypt']
  );
}

export async function decrypt(key, entry) {
  return crypto.subtle.decrypt({ name: 'AES-GCM', iv: decode64(entry.iv) }, key, decode64(entry.data));
}

export async function readCatalog(key, index) {
  const books = JSON.parse(decoder.decode(await decrypt(key, index.catalog)));
  if (!Array.isArray(books) || books.some(book => !/^[a-f0-9]{32}$/.test(book.id) || typeof book.title !== 'string')) {
    throw new Error('Invalid catalog.');
  }
  return books;
}

const dbName = 'private-epub-reader';
function openDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(dbName, 1);
    request.onupgradeneeded = () => request.result.createObjectStore('keys');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function keyStore(mode, action) {
  const db = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = db.transaction('keys', mode);
      const request = action(transaction.objectStore('keys'));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  } finally { db.close(); }
}

export const loadKey = () => keyStore('readonly', store => store.get('library'));
export const saveKey = key => keyStore('readwrite', store => store.put(key, 'library'));
export const forgetKey = () => keyStore('readwrite', store => store.delete('library'));