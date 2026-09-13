import { describe, expect, it, vi } from "vitest";
import { IS_MAINNET } from "./config";
import { connectDevKey } from "./devWallet";
import { isExplicitWalletRejection, subscribeWalletProviderEvents, type Wallet } from "./evm";

const DEMO_KEY = "0x" + "00".repeat(31) + "01";

describe("private-key signer boundary", () => {
  it("is available only in the testnet build", () => {
    if (IS_MAINNET) {
      expect(() => connectDevKey(DEMO_KEY)).toThrow("excluded from mainnet builds");
    } else {
      expect(connectDevKey(DEMO_KEY)).toMatchObject({ kind: "devkey", walletName: "Dev key" });
    }
  });

  it("only treats canonical wallet rejection as proof no write was authorized", () => {
    expect(isExplicitWalletRejection({ code: 4001 })).toBe(true);
    expect(isExplicitWalletRejection({ cause: { name: "UserRejectedRequestError" } })).toBe(true);
    expect(isExplicitWalletRejection(new Error("provider disconnected after request"))).toBe(false);
  });

});

function fakeProvider() {
  const listeners = new Map<string, Set<() => void>>();
  return {
    on(event: string, handler: () => void) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(handler);
    },
    removeListener(event: string, handler: () => void) {
      listeners.get(event)?.delete(handler);
    },
    emit(event: string) {
      for (const handler of listeners.get(event) ?? []) handler();
    },
    listenerCount(event: string): number {
      return listeners.get(event)?.size ?? 0;
    },
  };
}

function walletWithProvider(provider: unknown, address: `0x${string}` = "0x1111111111111111111111111111111111111111"): Wallet {
  return {
    address,
    kind: "injected",
    walletName: "Test wallet",
    client: {} as Wallet["client"],
    account: address,
    provider,
  };
}

describe("subscribeWalletProviderEvents", () => {
  it("fires the callback on accountsChanged, chainChanged, and disconnect", () => {
    const provider = fakeProvider();
    const wallet = walletWithProvider(provider);
    const onChange = vi.fn();
    subscribeWalletProviderEvents(wallet, onChange);

    provider.emit("accountsChanged");
    provider.emit("chainChanged");
    provider.emit("disconnect");

    expect(onChange).toHaveBeenCalledTimes(3);
  });

  it("stops firing once unsubscribed, and removes every listener it registered", () => {
    const provider = fakeProvider();
    const wallet = walletWithProvider(provider);
    const onChange = vi.fn();
    const unsubscribe = subscribeWalletProviderEvents(wallet, onChange);

    unsubscribe();
    provider.emit("accountsChanged");
    provider.emit("chainChanged");
    provider.emit("disconnect");

    expect(onChange).not.toHaveBeenCalled();
    expect(provider.listenerCount("accountsChanged")).toBe(0);
    expect(provider.listenerCount("chainChanged")).toBe(0);
    expect(provider.listenerCount("disconnect")).toBe(0);
  });

  it("returns a no-op unsubscribe for a wallet with no event-capable provider (e.g. the dev-key signer)", () => {
    const wallet = walletWithProvider(undefined);
    const onChange = vi.fn();
    const unsubscribe = subscribeWalletProviderEvents(wallet, onChange);
    expect(() => unsubscribe()).not.toThrow();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("scopes subscriptions to one wallet's own provider: a second wallet's events never cross-fire, and replacing the wallet only unsubscribes the old provider", () => {
    const providerA = fakeProvider();
    const providerB = fakeProvider();
    const walletA = walletWithProvider(providerA, "0x1111111111111111111111111111111111111111");
    const walletB = walletWithProvider(providerB, "0x2222222222222222222222222222222222222222");
    const onChangeA = vi.fn();
    const onChangeB = vi.fn();

    const unsubscribeA = subscribeWalletProviderEvents(walletA, onChangeA);
    subscribeWalletProviderEvents(walletB, onChangeB);

    // Explicit reconnect to a different wallet: unsubscribe the old provider first.
    unsubscribeA();
    providerA.emit("accountsChanged");
    providerB.emit("accountsChanged");

    expect(onChangeA).not.toHaveBeenCalled();
    expect(onChangeB).toHaveBeenCalledTimes(1);
  });
});
