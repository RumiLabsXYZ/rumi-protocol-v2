import { redirect } from '@sveltejs/kit';

// Telemetry moved under the Explorer tab (2026-09-15). Keep the old top-level
// route as a permanent redirect so shared links and bookmarks still land on the
// page instead of hitting the SPA fallback.
export const load = () => {
  throw redirect(308, '/explorer/telemetry');
};
