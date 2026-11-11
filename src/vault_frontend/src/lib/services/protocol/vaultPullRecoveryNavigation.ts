/** Explicit, owner-scoped navigation to existing read-only recovery. */
export const VAULT_PULL_REVIEW_EVENT = 'rumi:vault-pull-recovery-review';

export function isVaultPullRecoveryError(message: string): boolean {
  return /^(?:Vault operation|Saved vault operation) [0-9]+ /.test(message)
    || message.startsWith('A saved vault operation could not be verified.');
}

export function requestSavedVaultReview(owner: string): void {
  if (owner && typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(VAULT_PULL_REVIEW_EVENT, {detail: {owner}}));
  }
}
