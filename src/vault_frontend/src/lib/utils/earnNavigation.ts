// Thin, mockable wrapper around SvelteKit's client-side navigation used to
// land a fresh `/earn` visit on its resolved destination.
//
// `goto(..., { replaceState: true })` swaps the current history entry rather
// than pushing a new one, so `/earn` never becomes a Back-button trap between
// the two pool routes, and (unlike a full page reload) keeps the
// already-fetched rate snapshot alive for the tab bar that renders next.
import { goto } from '$app/navigation';

export function replaceRoute(path: string): Promise<void> {
  return goto(path, { replaceState: true });
}
