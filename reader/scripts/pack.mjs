import { createCipheriv, createDecipheriv, createHmac, pbkdf2Sync, randomBytes } from 'node:crypto';
import { mkdir, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { resolve, join, extname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(import.meta.url), '..', '..');
const input = join(root, 'books-private');
const output = join(root, 'public', 'books');
const passphrase = process.env.READER_PASSPHRASE;
if (!passphrase || passphrase.length < 12) {
  console.error('Set READER_PASSPHRASE to a strong passphrase (at least 12 characters). See reader/README.md.');
  process.exit(1);
}

const b64 = bytes => Buffer.from(bytes).toString('base64');
const unb64 = value => Buffer.from(value, 'base64');
const encrypt = (key, plaintext) => {
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  const data = Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]);
  return { iv: b64(iv), data: b64(data) };
};
const decrypt = (key, entry) => {
  const data = unb64(entry.data);
  const decipher = createDecipheriv('aes-256-gcm', key, unb64(entry.iv));
  decipher.setAuthTag(data.subarray(-16));
  return Buffer.concat([decipher.update(data.subarray(0, -16)), decipher.final()]);
};

try {
  const files = (await readdir(input)).filter(file => extname(file).toLowerCase() === '.epub').sort();
  if (!files.length) throw new Error(`No EPUBs found in ${input}. Add your books there first.`);
  let existing;
  try { existing = JSON.parse(await readFile(join(output, 'index.json'), 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (existing && existing.version !== 1) throw new Error('Unsupported existing catalog format.');
  const salt = existing ? unb64(existing.salt) : randomBytes(16);
  const key = pbkdf2Sync(passphrase, salt, 600000, 32, 'sha256');
  if (existing) {
    try { decrypt(key, existing.catalog); }
    catch { throw new Error('Passphrase does not match the existing library. No books were changed.'); }
  }
  const stage = join(root, 'public', `books-staging-${process.pid}`);
  await mkdir(stage, { recursive: true });
  try {
    const catalog = [];
    for (const file of files) {
      const bytes = await readFile(join(input, file));
      if (bytes.toString('ascii', 0, 2) !== 'PK') throw new Error(`${file} is not a ZIP-based EPUB.`);
      const id = createHmac('sha256', key).update(file).digest('hex').slice(0, 32);
      const title = basename(file, extname(file));
      let metadata = {};
      try {
        const record = JSON.parse(await readFile(join(input, `${basename(file, extname(file))}.metadata.json`), 'utf8'));
        const fields = ['title', 'author', 'original_title', 'synopsis', 'source_language', 'publication_status'];
        for (const field of fields) if (typeof record[field] === 'string') metadata[field] = record[field];
        for (const field of ['tags', 'genres', 'raw_tags_en', 'raw_tags_ko']) {
          if (Array.isArray(record[field])) metadata[field] = record[field].filter(value => typeof value === 'string');
        }
        if (typeof record.cover_file === 'string' && record.cover_file === `${basename(file, extname(file))}.cover.${record.cover_file.split('.').at(-1)}`) {
          const extension = extname(record.cover_file).toLowerCase();
          const mime = { '.jpg': 'jpeg', '.jpeg': 'jpeg', '.png': 'png', '.gif': 'gif', '.webp': 'webp' }[extension];
          if (mime) {
            const coverBytes = await readFile(join(input, record.cover_file));
            if (coverBytes.length > 20 * 1024 * 1024) throw new Error('Cover exceeds 20 MB.');
            metadata.cover = `data:image/${mime};base64,${b64(coverBytes)}`;
          }
        }
      } catch (error) { if (error.code !== 'ENOENT') throw new Error(`Invalid metadata for ${file}: ${error.message}`); }
      const encrypted = encrypt(key, bytes);
      await writeFile(join(stage, `${id}.json`), JSON.stringify(encrypted));
      catalog.push({ id, title, ...metadata });
      console.log(`Encrypted: ${title}`);
    }
    const catalogData = encrypt(key, Buffer.from(JSON.stringify(catalog)));
    await writeFile(join(stage, 'index.json'), JSON.stringify({ version: 1, salt: b64(salt), catalog: catalogData }));
    // Write everything before touching the old output. No plaintext book is placed in public/.
    await rm(output, { recursive: true, force: true });
    await rename(stage, output);
    console.log(`Packed ${files.length} book(s) into reader/public/books/`);
  } catch (error) {
    await rm(stage, { recursive: true, force: true });
    throw error;
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}