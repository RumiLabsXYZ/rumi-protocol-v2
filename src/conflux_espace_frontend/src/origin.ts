import { Principal } from "@dfinity/principal";

export type PublicOriginContext = "local-verification" | "deployment-verification" | "deployment";

export const LOCAL_PUBLIC_CANONICAL_ORIGIN = "http://127.0.0.1:5174";
export const DEPLOYMENT_VERIFICATION_CANISTER_PRINCIPAL = "rrkah-fqaaa-aaaaa-aaaaq-cai";
export const DEPLOYMENT_VERIFICATION_CANONICAL_ORIGIN =
  `https://${DEPLOYMENT_VERIFICATION_CANISTER_PRINCIPAL}.icp0.io`;

const CERTIFIED_CANISTER_ORIGIN = /^https:\/\/([a-z0-9-]+)\.icp0\.io$/;

function deploymentCanisterOrigin(value: string, context: PublicOriginContext): string {
  const match = CERTIFIED_CANISTER_ORIGIN.exec(value);
  if (!match) {
    throw new Error(
      "A deployable production-public origin must be exactly https://<canister-principal>.icp0.io with no port, path, query, raw gateway, custom domain, or IP host.",
    );
  }

  const principalText = match[1]!;
  let principal: Principal;
  try {
    principal = Principal.fromText(principalText);
  } catch {
    throw new Error("The production-public icp0.io hostname must contain a valid canonical Principal.");
  }
  if (principal.toText() !== principalText) {
    throw new Error("The production-public icp0.io hostname must contain a canonical lowercase Principal.");
  }
  if (principal.compareTo(Principal.managementCanister()) === "eq" || principal.isAnonymous()) {
    throw new Error("The management and anonymous Principals are reserved and cannot host the public frontend.");
  }
  const bytes = principal.toUint8Array();
  if (bytes.length < 2 || bytes[bytes.length - 1] !== 1) {
    throw new Error("The production-public hostname must identify a non-reserved opaque canister Principal.");
  }
  if (context === "deployment-verification") {
    if (principalText !== DEPLOYMENT_VERIFICATION_CANISTER_PRINCIPAL) {
      throw new Error("Deployment verification is locked to its deterministic test canister Principal.");
    }
  } else if (principalText === DEPLOYMENT_VERIFICATION_CANISTER_PRINCIPAL) {
    throw new Error("The deterministic verification canister Principal is denylisted for deployable artifacts.");
  }
  return value;
}

function parsedOrigin(value: string): URL {
  if (!value) throw new Error("VITE_PUBLIC_CANONICAL_ORIGIN is required for production-public builds.");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("VITE_PUBLIC_CANONICAL_ORIGIN must be an absolute origin URL.");
  }
  if (value !== url.origin || url.username || url.password) {
    throw new Error("VITE_PUBLIC_CANONICAL_ORIGIN must contain only an exact scheme and host; only local verification may include its fixed port.");
  }
  return url;
}

export function resolvePublicCanonicalOrigin(
  deploymentMode: string | undefined,
  value: string | undefined,
  context: string | undefined,
): string | null {
  if (deploymentMode !== "production-public") return null;
  if (context !== "local-verification" && context !== "deployment-verification" && context !== "deployment") {
    throw new Error("VITE_PUBLIC_ORIGIN_CONTEXT must be local-verification, deployment-verification, or deployment.");
  }
  const url = parsedOrigin(value ?? "");

  if (context === "local-verification") {
    if (url.origin !== LOCAL_PUBLIC_CANONICAL_ORIGIN) {
      throw new Error(`Local production-public verification is locked to ${LOCAL_PUBLIC_CANONICAL_ORIGIN}.`);
    }
    return url.origin;
  }

  return deploymentCanisterOrigin(url.origin, context);
}

export function publicOriginRefusal(canonicalOrigin: string | null, currentOrigin: string): string | null {
  if (!canonicalOrigin) return "This production-public build has no canonical origin.";
  return currentOrigin === canonicalOrigin
    ? null
    : `This is not the canonical production origin. Open ${canonicalOrigin} before connecting a wallet.`;
}
