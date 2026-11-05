import { Actor, HttpAgent } from "@dfinity/agent";
import { LOCAL_IC_HOST } from "../localNetwork";

// In-memory cache for anonymous actors
const actorCache = new Map<string, any>();

export const createAnonymousActorHelper = (canisterId: string, idl: any) => {
  const cacheKey = `${canisterId}-${idl.name || "anonymous"}`;
  if (actorCache.has(cacheKey)) {
    return actorCache.get(cacheKey);
  }

  const agent = new HttpAgent({
    host:
      process.env.DFX_NETWORK !== "ic"
        ? LOCAL_IC_HOST
        : "https://icp0.io",
  });

  if (process.env.DFX_NETWORK !== "ic") {
    agent.fetchRootKey();
  }

  const actor = Actor.createActor(idl as any, {
    agent,
    canisterId,
  });

  actorCache.set(cacheKey, actor);
  return actor;
};