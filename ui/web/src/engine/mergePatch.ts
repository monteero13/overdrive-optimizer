// RFC 7396 JSON Merge Patch. Arrays are replaced whole; null deletes a key.
export function applyMergePatch<T>(target: T, patch: unknown): T {
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) return patch as T;
  const base: Record<string, unknown> =
    target !== null && typeof target === 'object' && !Array.isArray(target)
      ? { ...(target as Record<string, unknown>) }
      : {};
  for (const [k, v] of Object.entries(patch as Record<string, unknown>)) {
    if (v === null) delete base[k];
    else base[k] = applyMergePatch(base[k], v);
  }
  return base as T;
}
