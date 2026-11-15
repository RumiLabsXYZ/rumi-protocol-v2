type WalletSessionTransitionListener = (generation: number) => void;

const listeners = new Set<WalletSessionTransitionListener>();

export function registerWalletSessionTransitionListener(listener: WalletSessionTransitionListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function notifyWalletSessionTransition(generation: number): void {
  for (const listener of listeners) listener(generation);
}
