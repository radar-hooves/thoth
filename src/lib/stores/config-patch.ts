/** Wire-shape value: what `get_config` returns and `patch_config` accepts. */
type Wire = { [key: string]: unknown };

function isObject(value: unknown): value is Wire {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The minimal nested patch that turns `before` into `after`.
 *
 * Objects recurse; every other value (primitives, null, arrays) is a leaf,
 * compared whole and sent whole. Keys listed in `skip` (dotted paths) never
 * appear. Returns null when nothing changed.
 */
export function buildPatch(
  beforeDoc: object,
  afterDoc: object,
  skip: readonly string[] = [],
  path = '',
): Wire | null {
  const before = beforeDoc as Wire;
  const after = afterDoc as Wire;
  const patch: Wire = {};
  for (const key of Object.keys(after)) {
    const here = path ? `${path}.${key}` : key;
    if (skip.includes(here)) continue;
    const prev = before[key];
    const next = after[key];
    if (isObject(prev) && isObject(next)) {
      const sub = buildPatch(prev, next, skip, here);
      if (sub) patch[key] = sub;
    } else if (JSON.stringify(prev) !== JSON.stringify(next)) {
      patch[key] = next;
    }
  }
  return Object.keys(patch).length > 0 ? patch : null;
}
