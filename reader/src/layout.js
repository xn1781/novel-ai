export function layoutFor(flow) {
  return flow === 'scrolled-continuous'
    ? { manager: 'continuous', flow: 'scrolled-continuous' }
    : { manager: 'default', flow: flow === 'paginated' ? 'paginated' : 'scrolled-doc' };
}