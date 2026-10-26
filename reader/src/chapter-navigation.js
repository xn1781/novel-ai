// epub.js calls each item in the reading-order spine a section. Sections are
// usually chapters, though an EPUB can split a chapter across several items.
export function adjacentSection(spine, index, direction) {
  if (!Number.isInteger(index) || !spine) return null;
  const section = spine.get(index);
  return (direction === 'next' ? section?.next() : section?.prev()) || null;
}