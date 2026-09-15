import { AnonymousIdentity, Actor, HttpAgent } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { idlFactory } from '$declarations/rumi_cycle_sentinel';
import type {
  _SERVICE,
  AuthenticatedQueryError,
  FundingOperation,
  GlobalPolicyArgs,
  ProposalRecord,
  PublicAlarm,
  PublicOverview,
  PublicTargetRow,
  Result_3,
  Result_4,
  Result_9,
  TargetArgs,
  TargetFundingPolicy,
  TargetPatch,
} from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';
import { auth } from './auth';
import { CONFIG, CANISTER_IDS, isCycleSentinelConfigured } from '../config';
export { isCycleSentinelConfigured } from '../config';

export type SentinelActor = _SERVICE;
export type Cursor = [] | [string];
export type TelemetrySnapshot = {
  overview: PublicOverview;
  targets: PublicTargetRow[];
  alarms: PublicAlarm[];
  refreshedAt: Date;
};
export const PAGE_SIZE = 100;

type AuthenticatedPage<T> = { Ok: { items: T[]; next_cursor: Cursor } } | { Err: AuthenticatedQueryError };
type Variant = Record<string, unknown>;

export function canisterError(result: { Err: unknown }): never {
  const variant = result.Err;
  const name = typeof variant === 'string' ? variant : variant && typeof variant === 'object' ? Object.keys(variant)[0] : undefined;
  throw new Error(`Cycle Sentinel rejected request: ${name ?? 'Unknown'}`);
}

function resultOk<T>(result: { Ok: T } | { Err: unknown }): T {
  if ('Err' in result) canisterError(result);
  return result.Ok;
}

function requireConfiguredId(id: string): string {
  const trimmed = id.trim();
  if (!trimmed || trimmed === 'aaaaa-aa') throw new Error('Cycle Sentinel is not configured yet.');
  return trimmed;
}

export function createAnonymousSentinelActor(id?: string): SentinelActor {
  const canisterId = requireConfiguredId(id ?? CANISTER_IDS.CYCLE_SENTINEL);
  const agent = new HttpAgent({
    host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://ic0.app',
    identity: new AnonymousIdentity(),
  });
  if (CONFIG.isLocal) agent.fetchRootKey().catch(() => undefined);
  return Actor.createActor(idlFactory, { agent, canisterId }) as SentinelActor;
}

export async function createAuthenticatedSentinelActor(): Promise<SentinelActor> {
  if (!isCycleSentinelConfigured) throw new Error('Cycle Sentinel is not configured yet.');
  return auth.getActor<SentinelActor>(CANISTER_IDS.CYCLE_SENTINEL, idlFactory);
}

async function collect<T>(
  load: (cursor: Cursor, size: number) => Promise<AuthenticatedPage<T>>,
  label: string,
): Promise<T[]> {
  const items: T[] = [];
  const seen = new Set<string>();
  let cursor: Cursor = [];
  for (;;) {
    const result = await load(cursor, PAGE_SIZE);
    if ('Err' in result) canisterError(result);
    items.push(...result.Ok.items);
    if (result.Ok.next_cursor.length === 0) return items;
    const next = result.Ok.next_cursor[0];
    if (seen.has(next)) throw new Error(`Cycle Sentinel returned a repeated ${label} cursor.`);
    seen.add(next);
    cursor = [next];
  }
}

async function collectPublic<T>(
  load: (cursor: Cursor, size: number) => Promise<{ Ok: { items: T[]; next_cursor: Cursor } } | { Err: Variant }>,
  label: string,
): Promise<T[]> {
  const items: T[] = [];
  const seen = new Set<string>();
  let cursor: Cursor = [];
  for (;;) {
    const result = await load(cursor, PAGE_SIZE);
    if ('Err' in result) canisterError(result);
    items.push(...result.Ok.items);
    if (result.Ok.next_cursor.length === 0) return items;
    const next = result.Ok.next_cursor[0];
    if (seen.has(next)) throw new Error(`Cycle Sentinel returned a repeated ${label} cursor.`);
    seen.add(next);
    cursor = [next];
  }
}

export async function loadPublicTelemetry(actor: SentinelActor = createAnonymousSentinelActor()): Promise<TelemetrySnapshot> {
  const [overview, targets, alarms] = await Promise.all([
    actor.get_public_overview(),
    collectPublic(actor.list_public_targets.bind(actor), 'target'),
    collectPublic(actor.list_public_alarms.bind(actor), 'alarm'),
  ]);
  return { overview, targets, alarms, refreshedAt: new Date() };
}

export async function getPermissions(actor: SentinelActor): Promise<{ is_signer: boolean }> {
  const result: Result_3 = await actor.get_my_permissions();
  if ('Err' in result) {
    if ('NotSigner' in result.Err) return { is_signer: false };
    canisterError(result);
  }
  return result.Ok;
}

export function listProposals(actor: SentinelActor): Promise<ProposalRecord[]> {
  return collect<ProposalRecord>(
    (cursor, size): Promise<Result_4> => actor.list_governance_proposals(cursor, size),
    'proposal',
  );
}

export function listUnresolvedFundingOperations(actor: SentinelActor): Promise<FundingOperation[]> {
  return collect<FundingOperation>(
    (cursor, size): Promise<Result_9> => actor.list_unresolved_funding_operations(cursor, size),
    'funding operation',
  );
}

export function parsePrincipal(value: string, label: string): Principal {
  const text = value.trim();
  if (!text) throw new Error(`${label} is required.`);
  if (text === 'aaaaa-aa') throw new Error(`${label} cannot be aaaaa-aa.`);
  try {
    const principal = Principal.fromText(text);
    if (principal.isAnonymous()) throw new Error(`${label} cannot be anonymous.`);
    return principal;
  } catch (error) {
    if (error instanceof Error && error.message.includes('cannot be anonymous')) throw error;
    throw new Error(`${label} must be a valid principal.`);
  }
}

export function parseNat(value: string, label: string): bigint {
  const text = value.trim();
  if (!/^\d+$/.test(text)) throw new Error(`${label} must be a non-negative integer.`);
  return BigInt(text);
}

export function parseNat32(value: string, label: string): number {
  const parsed = parseNat(value, label);
  if (parsed > 4_294_967_295n) throw new Error(`${label} exceeds nat32.`);
  return Number(parsed);
}

export function requireText(value: string, label: string): string {
  const text = value.trim();
  if (!text) throw new Error(`${label} is required.`);
  return text;
}

function assertTargetArgs(args: TargetArgs): void {
  if (args.principal.isAnonymous()) throw new Error('Target principal cannot be anonymous.');
  requireText(args.display_name, 'Display name');
  requireText(args.project, 'Project');
  args.tags.forEach((tag, index) => requireText(tag, `Tag ${index + 1}`));
  assertFundingPolicy(args.funding_policy);
}

function assertFundingPolicy(policy: TargetFundingPolicy): void {
  if (policy.low_balance_threshold_cycles < 1n) throw new Error('Low balance threshold must be positive.');
  if (policy.refill_cycles < 1n) throw new Error('Refill cycles must be positive.');
  if (policy.daily_cap_cycles < 1n) throw new Error('Daily cap must be positive.');
  if (policy.burn_anomaly_limit_cycles_per_day.some((value) => value < 1n)) throw new Error('Burn anomaly limit must be positive when supplied.');
}

function assertNat(value: bigint, label: string): void {
  if (value < 0n) throw new Error(`${label} must be a non-negative integer.`);
}

function assertPatch(patch: TargetPatch): void {
  patch.display_name.forEach((value) => requireText(value, 'Display name'));
  patch.project.forEach((value) => requireText(value, 'Project'));
  patch.tags.forEach((tags) => tags.forEach((tag, index) => requireText(tag, `Tag ${index + 1}`)));
  patch.funding_policy.forEach(assertFundingPolicy);
}

export const sentinelManagement = {
  proposeAddSigner(actor: SentinelActor, principal: Principal) {
    if (principal.isAnonymous()) throw new Error('Signer principal cannot be anonymous.');
    return actor.propose_add_signer(principal).then(resultOk);
  },
  proposeRemoveSigner(actor: SentinelActor, principal: Principal) {
    if (principal.isAnonymous()) throw new Error('Signer principal cannot be anonymous.');
    return actor.propose_remove_signer(principal).then(resultOk);
  },
  proposeSetSignerThreshold(actor: SentinelActor, threshold: number) {
    if (!Number.isInteger(threshold) || threshold < 1 || threshold > 4_294_967_295) throw new Error('Signer threshold must be a positive nat32.');
    return actor.propose_set_signer_threshold(threshold).then(resultOk);
  },
  proposeRegisterTarget(actor: SentinelActor, args: TargetArgs) {
    assertTargetArgs(args);
    return actor.propose_register_target(args).then(resultOk);
  },
  proposeUpdateTarget(actor: SentinelActor, principal: Principal, patch: TargetPatch) {
    if (principal.isAnonymous()) throw new Error('Target principal cannot be anonymous.');
    assertPatch(patch);
    return actor.propose_update_target(principal, patch).then(resultOk);
  },
  proposeRemoveTarget(actor: SentinelActor, principal: Principal) {
    if (principal.isAnonymous()) throw new Error('Target principal cannot be anonymous.');
    return actor.propose_remove_target(principal).then(resultOk);
  },
  proposeSetGlobalPolicy(actor: SentinelActor, policy: GlobalPolicyArgs) {
    if (policy.sample_interval_secs < 1n || policy.stale_after_secs < 1n) throw new Error('Sample interval and stale-after must be positive.');
    if (policy.global_daily_cap_cycles < 1n) throw new Error('Global daily cap must be positive.');
    if (policy.min_icp_reserve_e8s < 0n) throw new Error('ICP reserve must be non-negative.');
    if ([policy.self_recovery_policy.refill_cycles, policy.self_recovery_policy.low_balance_threshold_cycles, policy.self_recovery_policy.daily_cap_cycles, policy.self_recovery_policy.protected_reserve_cycles].some((value) => value < 1n)) throw new Error('Self-recovery policy fields must be positive.');
    if ([policy.timelocks.unpause_secs, policy.timelocks.spend_policy_secs, policy.timelocks.target_registry_secs, policy.timelocks.signer_change_secs].some((value) => value < 1n)) throw new Error('Governance timelocks must be positive.');
    return actor.propose_set_global_policy(policy).then(resultOk);
  },
  proposeUnpauseTarget(actor: SentinelActor, principal: Principal) {
    if (principal.isAnonymous()) throw new Error('Target principal cannot be anonymous.');
    return actor.propose_unpause_target(principal).then(resultOk);
  },
  approveProposal(actor: SentinelActor, id: bigint) { assertNat(id, 'Proposal ID'); return actor.approve_proposal(id).then(resultOk); },
  executeProposal(actor: SentinelActor, id: bigint) { assertNat(id, 'Proposal ID'); return actor.execute_proposal(id).then(resultOk); },
  cancelProposal(actor: SentinelActor, id: bigint) { assertNat(id, 'Proposal ID'); return actor.cancel_proposal(id).then(resultOk); },
  acknowledgeAlarm(actor: SentinelActor, id: bigint) { assertNat(id, 'Alarm ID'); return actor.acknowledge_alarm(id).then(resultOk); },
  pauseTarget(actor: SentinelActor, target: Principal) { if (target.isAnonymous()) throw new Error('Target principal cannot be anonymous.'); return actor.pause_target(target).then(resultOk); },
  manualTopUp(actor: SentinelActor, target: Principal): Promise<FundingOperation> { if (target.isAnonymous()) throw new Error('Target principal cannot be anonymous.'); return actor.manual_top_up(target).then(resultOk); },
  attachBlockProof(actor: SentinelActor, operation: bigint, block: bigint): Promise<FundingOperation> { assertNat(operation, 'Operation ID'); assertNat(block, 'Block index'); return actor.attach_block_proof(operation, block).then(resultOk); },
  attachRefundBlockProof(actor: SentinelActor, operation: bigint, block: bigint): Promise<FundingOperation> { assertNat(operation, 'Operation ID'); assertNat(block, 'Block index'); return actor.attach_refund_block_proof(operation, block).then(resultOk); },
  resolveUnknownAsSpent(actor: SentinelActor, operation: bigint): Promise<FundingOperation> { assertNat(operation, 'Operation ID'); return actor.resolve_unknown_as_spent(operation).then(resultOk); },
};

export type { AuthenticatedQueryError, FundingOperation, GlobalPolicyArgs, ProposalRecord, PublicAlarm, PublicOverview, PublicTargetRow, TargetArgs, TargetFundingPolicy, TargetPatch };
