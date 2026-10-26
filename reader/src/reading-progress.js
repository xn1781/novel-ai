export function normalizedProgress(value) {
  const data = value && typeof value === 'object' ? value : {};
  return {
    cfi: typeof data.cfi === 'string' ? data.cfi : null,
    index: Number.isInteger(data.index) && data.index >= 0 ? data.index : null,
    scroll: typeof data.scroll === 'number' && Number.isFinite(data.scroll) ? Math.max(0, Math.min(1, data.scroll)) : null,
    read: Array.isArray(data.read) ? [...new Set(data.read.filter(index => Number.isInteger(index) && index >= 0))] : [],
  };
}

export function sortedChapters(chapters, read, order) {
  const entries = [...chapters];
  if (order === 'desc') entries.reverse();
  if (order === 'unread') entries.sort((a, b) => Number(read.includes(a.index)) - Number(read.includes(b.index)) || a.order - b.order);
  return entries;
}

export function scrollFraction(element) {
  if (!element) return null;
  const distance = element.scrollHeight - element.clientHeight;
  return distance > 0 ? Math.max(0, Math.min(1, element.scrollTop / distance)) : 0;
}