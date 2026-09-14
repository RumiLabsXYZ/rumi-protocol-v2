import { describe, expect, it } from "vitest";
import {
  DEPLOYMENT_VERIFICATION_CANONICAL_ORIGIN,
  LOCAL_PUBLIC_CANONICAL_ORIGIN,
  REVIEWED_PUBLIC_CANONICAL_ORIGIN,
  publicOriginRefusal,
  resolvePublicCanonicalOrigin,
} from "./origin";

const REVIEWED_CANISTER_ORIGIN = "https://a52ri-naaaa-aaaas-qgy4a-cai.icp0.io";

describe("production-public canonical origin", () => {
  it("accepts only the exact reviewed custom origin for deployment", () => {
    expect(resolvePublicCanonicalOrigin("production-public", REVIEWED_PUBLIC_CANONICAL_ORIGIN, "deployment"))
      .toBe(REVIEWED_PUBLIC_CANONICAL_ORIGIN);
    for (const origin of [
      REVIEWED_CANISTER_ORIGIN,
      "https://rumi.example",
      "https://8.8.8.8",
      "https://localhost",
      "https://tfesu-vyaaa-aaaap-qrd7a-cai.ic0.app",
      "https://tfesu-vyaaa-aaaap-qrd7a-cai.raw.icp0.io",
      "https://raw.a52ri-naaaa-aaaas-qgy4a-cai.icp0.io",
      "https://extra.a52ri-naaaa-aaaas-qgy4a-cai.icp0.io",
      "http://conflux.rumiprotocol.com",
      "https://conflux.rumiprotocol.com:8443",
    ]) expect(() => resolvePublicCanonicalOrigin("production-public", origin, "deployment")).toThrow("exactly");
  });

  it("rejects missing, malformed, noncanonical, path, query, and fragment values", () => {
    expect(() => resolvePublicCanonicalOrigin("production-public", undefined, "deployment")).toThrow("required");
    expect(() => resolvePublicCanonicalOrigin("production-public", "not a URL", "deployment")).toThrow("absolute origin");
    for (const origin of [
      "https://not-a-principal.icp0.io",
      "https://TFESU-VYAAA-AAAAP-QRD7A-CAI.icp0.io",
      `${REVIEWED_PUBLIC_CANONICAL_ORIGIN}/`,
      `${REVIEWED_PUBLIC_CANONICAL_ORIGIN}/app`,
      `${REVIEWED_PUBLIC_CANONICAL_ORIGIN}?source=alias`,
      `${REVIEWED_PUBLIC_CANONICAL_ORIGIN}#launch`,
      `${REVIEWED_PUBLIC_CANONICAL_ORIGIN}:443`,
      "https://user:pass@conflux.rumiprotocol.com",
      "https://conflux.rumiprotocol.com.evil.example",
      "https://evil.conflux.rumiprotocol.com",
    ]) expect(() => resolvePublicCanonicalOrigin("production-public", origin, "deployment")).toThrow();
  });

  it("rejects reserved, non-canister, and deterministic verification origins", () => {
    expect(() => resolvePublicCanonicalOrigin("production-public", "https://aaaaa-aa.icp0.io", "deployment"))
      .toThrow("exactly");
    expect(() => resolvePublicCanonicalOrigin("production-public", "https://2vxsx-fae.icp0.io", "deployment"))
      .toThrow("exactly");
    expect(() => resolvePublicCanonicalOrigin("production-public", "https://2ibo7-dia.icp0.io", "deployment"))
      .toThrow();
    expect(() => resolvePublicCanonicalOrigin(
      "production-public",
      DEPLOYMENT_VERIFICATION_CANONICAL_ORIGIN,
      "deployment",
    )).toThrow("exactly");
  });

  it("rejects reviewer IPv4 and IPv6 destinations by hostname construction", () => {
    for (const origin of [
      "https://127.0.0.1",
      "https://10.0.0.1",
      "https://169.254.1.2",
      "https://[::]",
      "https://[::1]",
      "https://[fe80::1]",
      "https://[fc00::1]",
      "https://[fd12:3456::1]",
      "https://[2001:db8::1]",
      "https://[2606:4700:4700::1111]",
      "https://[::ffff:7f00:1]",
      "https://[::ffff:127.0.0.1]",
    ]) expect(() => resolvePublicCanonicalOrigin("production-public", origin, "deployment")).toThrow();
  });

  it("keeps local and deployment verification contexts isolated from deployable artifacts", () => {
    expect(resolvePublicCanonicalOrigin("production-public", LOCAL_PUBLIC_CANONICAL_ORIGIN, "local-verification"))
      .toBe(LOCAL_PUBLIC_CANONICAL_ORIGIN);
    expect(() => resolvePublicCanonicalOrigin("production-public", LOCAL_PUBLIC_CANONICAL_ORIGIN, "deployment"))
      .toThrow();
    expect(resolvePublicCanonicalOrigin(
      "production-public",
      DEPLOYMENT_VERIFICATION_CANONICAL_ORIGIN,
      "deployment-verification",
    )).toBe(DEPLOYMENT_VERIFICATION_CANONICAL_ORIGIN);
    expect(() => resolvePublicCanonicalOrigin(
      "production-public",
      REVIEWED_PUBLIC_CANONICAL_ORIGIN,
      "deployment-verification",
    )).toThrow("locked");
    expect(resolvePublicCanonicalOrigin("testnet", undefined, undefined)).toBeNull();
  });

  it("blocks alternate certified origins before an origin-local empty lock store can enable writes", () => {
    const canonical = REVIEWED_PUBLIC_CANONICAL_ORIGIN;
    const locksByOrigin = new Map<string, string>([[canonical, "submitted-deposit-lock"]]);
    const alternateCanister = "https://ryjl3-tyaaa-aaaaa-aaaba-cai.icp0.io";
    expect(locksByOrigin.get(alternateCanister)).toBeUndefined();
    expect(publicOriginRefusal(canonical, canonical)).toBeNull();
    expect(publicOriginRefusal(canonical, alternateCanister)).toContain("not the canonical");
    expect(publicOriginRefusal(canonical, REVIEWED_CANISTER_ORIGIN))
      .toContain("not the canonical");
    expect(publicOriginRefusal(null, canonical)).toContain("no canonical origin");
  });
});
