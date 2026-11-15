import type { ActionBoundContext } from './walletOperations';

/** Run display-only follow-up work after a financial action has succeeded. */
export async function runConfirmedActionRefresh(
  actionLabel: string,
  ctx: ActionBoundContext,
  refresh: () => Promise<unknown>,
  report: (message: string) => void,
): Promise<boolean> {
  try {
    if (!ctx.assertCurrent()) throw new Error('wallet session changed');
    await refresh();
    return true;
  } catch {
    const message = ctx.assertCurrent()
      ? `${actionLabel} is confirmed, but the vault display could not refresh. Reconnect or refresh the vault status before relying on the displayed balance. Do not submit the financial action again.`
      : `${actionLabel} is confirmed. Reconnect the wallet that submitted it to refresh the vault status. Do not submit the financial action again.`;
    report(message);
    return false;
  }
}
