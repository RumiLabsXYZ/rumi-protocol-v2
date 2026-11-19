import assert from 'node:assert/strict';
export function effectiveContentType(properties) {
  const overrides = (properties.headers ?? []).filter(([name]) => name.toLowerCase() === 'content-type');
  assert.ok(overrides.length <= 1, 'Ambiguous Content-Type overrides');
  const type = overrides.length ? overrides[0][1] : properties.content_type;
  assert.ok(typeof type === 'string' && type.length > 0, 'Missing effective Content-Type');
  return type;
}
