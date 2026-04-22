import { redirect, type Load } from '@sveltejs/kit';

export const load: Load = ({ params }) => {
  throw redirect(301, `/explorer/e/event/dex:${params.source}:${params.id}`);
};
