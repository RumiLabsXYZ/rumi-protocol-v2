import { safeGetCanisterEnv } from "@icp-sdk/core/agent/canister-env";

export interface CanisterEnv {
  explorerBffId: string;
  isLocal: boolean;
}

export function getCanisterEnv(): CanisterEnv {
  const env = safeGetCanisterEnv<{ readonly ["PUBLIC_CANISTER_ID:explorer_bff"]: string }>();
  const explorerBffId = env?.["PUBLIC_CANISTER_ID:explorer_bff"];

  if (!explorerBffId) {
    throw new Error(
      "explorer_bff canister ID not found in ic_env cookie. " +
        "Ensure both canisters were deployed to the same environment.",
    );
  }

  const isLocal =
    typeof window !== "undefined" &&
    (window.location.hostname.endsWith(".localhost") ||
      window.location.hostname === "127.0.0.1" ||
      window.location.hostname === "localhost");

  return { explorerBffId, isLocal };
}
