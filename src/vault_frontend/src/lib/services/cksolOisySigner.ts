// Backward-compatible names for the existing ckSOL flow. All minters share
// the same per-action ownership/cleanup invariant and isolated popup transport.
export {
  beginMinterOisyAction as beginCksolOisyAction,
  getMinterOisySignerAgent as getCksolOisySignerAgent,
  type MinterOisyAction as CksolOisyAction,
} from './minterOisySigner';
