/**
 * Unwrap a Candid `opt T` (which the bindings represent as `[] | [T]`).
 * Returns `undefined` for None, the value for Some.
 */
export function unwrap<T>(opt: [] | [T] | undefined): T | undefined {
  if (!opt) return undefined;
  return opt.length > 0 ? opt[0] : undefined;
}

/**
 * True if a Candid `opt T` is Some.
 */
export function isSome<T>(opt: [] | [T] | undefined): opt is [T] {
  return !!opt && opt.length > 0;
}
