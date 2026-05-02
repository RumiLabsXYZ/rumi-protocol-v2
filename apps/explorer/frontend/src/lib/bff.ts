import { createActor } from "@/bindings/explorer_bff/explorer_bff";
import { getAgentOptions } from "./agent";
import { getCanisterEnv } from "./canisterEnv";

let cachedActor: ReturnType<typeof createActor> | null = null;

export function getBff() {
  if (cachedActor) return cachedActor;
  const env = getCanisterEnv();
  cachedActor = createActor(env.explorerBffId, {
    agentOptions: getAgentOptions(),
  });
  return cachedActor;
}
