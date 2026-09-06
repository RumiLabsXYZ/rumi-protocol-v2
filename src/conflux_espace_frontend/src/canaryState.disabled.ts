/**
 * Build-time production-public substitute. Canary storage keys, lifecycle
 * parsing, transaction recovery, and action policy are not shipped in the
 * public asset bundle. All runtime branches that could call these exports are
 * pinned false by Vite; inert values keep module linking fail-closed.
 */
export const canaryStorageKey = () => "";
export const newCanaryRecord = () => null;
export const newCanaryOpenLock = () => null;
export const productionLifecycleUsed = () => false;
export const isRecoverableOpenCandidate = () => false;
export const parseCanaryRecord = () => null;
export const recordTransaction = () => null;
export const replaceLatestTransactionHash = () => null;
export const markTransactionReceiptSucceeded = () => null;
export const pendingTransaction = () => null;
export const applyFailedTransactionFinality = () => null;
export const manualRecoveryTarget = () => null;
export const reconcileCanaryPhase = () => null;
export const validateCanaryAction = () => "restricted build action unavailable";
