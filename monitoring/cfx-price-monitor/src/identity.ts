import { readFileSync } from "node:fs";
import type { Identity } from "@dfinity/agent";
import { Secp256k1KeyIdentity } from "@dfinity/identity-secp256k1";

/**
 * Parse a secp256k1 EC-private-key PEM into an Identity. This is the format
 * `dfx identity export <name>` / `icp identity export` produces. The price-pusher
 * key should be a DEDICATED identity (never the developer key) registered via
 * `set_price_pusher_principal`.
 */
export function identityFromPem(pem: string): Identity {
  try {
    return Secp256k1KeyIdentity.fromPem(pem);
  } catch (err) {
    throw new Error(
      `failed to parse price-pusher identity PEM (expected a secp256k1 EC private key, e.g. from \`dfx identity export\`): ${
        err instanceof Error ? err.message : String(err)
      }`,
    );
  }
}

/** Read + parse the price-pusher identity from a PEM file path. */
export function loadIdentityFromFile(path: string): Identity {
  let pem: string;
  try {
    pem = readFileSync(path, "utf8");
  } catch (err) {
    throw new Error(`cannot read IDENTITY_PEM at ${path}: ${err instanceof Error ? err.message : String(err)}`);
  }
  return identityFromPem(pem);
}
