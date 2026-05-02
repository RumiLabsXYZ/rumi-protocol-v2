import { getCanisterEnv } from "./canisterEnv";

export function getAgentOptions() {
  const env = getCanisterEnv();
  return {
    host: env.isLocal ? "http://127.0.0.1:8000" : "https://icp-api.io",
    shouldFetchRootKey: env.isLocal,
  };
}
