import { sameVaultPullRequest, type VaultPullRecoverySnapshot, type VaultPullRequest } from './vaultOperationJournal';

export function exactTokenAmount(raw: bigint, decimals: number): string {
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 30) return `${raw} base units`;
  const factor = 10n ** BigInt(decimals);
  const fraction = (raw % factor).toString().padStart(decimals, '0').replace(/0+$/, '');
  return `${raw / factor}${fraction ? `.${fraction}` : ''}`;
}

export function savedVaultRequestSummary(request: VaultPullRequest, collateral?: { symbol: string; decimals: number }): string {
  const terms = 'OpenVault' in request ? request.OpenVault : 'OpenVaultAndBorrow' in request ? request.OpenVaultAndBorrow : request.AddMargin;
  const amount = collateral ? `${exactTokenAmount(terms.amount_e8s, collateral.decimals)} ${collateral.symbol}` : `${terms.amount_e8s} collateral base units`;
  if ('OpenVaultAndBorrow' in request) return `Open a vault with ${amount} and borrow ${exactTokenAmount(request.OpenVaultAndBorrow.borrow_amount_e8s, 8)} icUSD`;
  if ('OpenVault' in request) return `Open a vault with ${amount}`;
  return `Add ${amount} to vault #${request.AddMargin.vault_id}`;
}

export function savedVaultRequestActions(snapshot: VaultPullRecoverySnapshot) {
  const intent = snapshot.intent;
  const held = (message: string) => ({ canRestore: false, canCancel: false, retired: false, message });
  if (!intent) return held('No browser-saved request is available.');
  const id = BigInt(intent.operationId);
  if (snapshot.status.acknowledged_through >= id) return {
    canRestore: false, canCancel: false, retired: true,
    message: 'This request was already retired. It may have completed elsewhere. Refresh vaults and balances before starting another request.',
  };
  const active = snapshot.status.active[0];
  if (active && (active.operation_id !== id || !sameVaultPullRequest(active.request, intent.request))) {
    return held('The backend has a different request. The saved request remains held; refresh its status or contact support.');
  }
  if (!active) return id === snapshot.status.acknowledged_through + 1n
    ? { canRestore: true, canCancel: true, retired: false, message: 'Review the saved amounts. Restore to continue this exact request, or ask the backend to retire it safely.' }
    : held('This saved request does not match the backend sequence. Keep it for support; do not start another transfer.');
  if (active.confirmed_wide_block_index?.[0] !== undefined || 'Submitted' in active.phase) {
    return held('A transfer may already have happened. Verify its exact receipt instead of cancelling or sending again.');
  }
  return {
    canRestore: true,
    canCancel: 'Prepared' in active.phase || 'SafeNoEffect' in active.phase,
    retired: false,
    message: 'Prepared' in active.phase || 'SafeNoEffect' in active.phase
      ? 'Restore this exact request to continue, or cancel only if the backend confirms it is safe.'
      : 'Collateral or a completed result is already recorded. Restore to finish recovery; do not send collateral again.',
  };
}
