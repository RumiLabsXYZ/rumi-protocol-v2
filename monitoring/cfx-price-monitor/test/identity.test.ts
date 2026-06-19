import { describe, it, expect } from "vitest";
import crypto from "node:crypto";
import { identityFromPem } from "../src/identity.js";

function makeSecp256k1Pem(): string {
  const { privateKey } = crypto.generateKeyPairSync("ec", { namedCurve: "secp256k1" });
  return privateKey.export({ type: "sec1", format: "pem" }).toString();
}

describe("identityFromPem", () => {
  it("loads a secp256k1 EC private key PEM into a usable identity", () => {
    const id = identityFromPem(makeSecp256k1Pem());
    expect(id.getPrincipal().isAnonymous()).toBe(false);
  });

  it("is deterministic for the same PEM (stable principal)", () => {
    const pem = makeSecp256k1Pem();
    expect(identityFromPem(pem).getPrincipal().toText()).toBe(identityFromPem(pem).getPrincipal().toText());
  });

  it("throws a helpful error on a malformed PEM", () => {
    expect(() => identityFromPem("not a pem")).toThrow(/secp256k1|PEM/i);
  });
});
