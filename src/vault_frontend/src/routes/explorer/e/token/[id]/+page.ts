import { error, type Load } from '@sveltejs/kit';
import { resolveTokenAlias, isPrincipal, KNOWN_TOKENS } from '$utils/explorerHelpers';

/**
 * Resolve `[id]` param to a canister principal text. Accepts:
 *   - well-known symbol aliases ("icusd", "3usd", "icp", "ckusdt", "ckusdc", "3pool")
 *   - any principal text (validated by `isPrincipal`)
 *
 * The page itself handles unknown principals by rendering a stub identity card,
 * so we only error here on syntactically invalid input.
 */
export const load: Load = ({ params }) => {
  const raw = (params.id ?? '').trim();
  if (!raw) throw error(400, 'Missing token id');

  const aliased = resolveTokenAlias(raw);
  if (aliased) {
    return { tokenPrincipal: aliased, requestedAlias: raw.toLowerCase() };
  }

  if (raw in KNOWN_TOKENS) {
    return { tokenPrincipal: raw, requestedAlias: null };
  }

  if (isPrincipal(raw)) {
    return { tokenPrincipal: raw, requestedAlias: null };
  }

  throw error(404, `Unknown token: "${raw}"`);
};
