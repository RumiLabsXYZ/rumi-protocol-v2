import { describe, expect, it, vi } from 'vitest';
import type { _SERVICE, PublicOverview, ProposalRecord } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';
import {
  canisterError,
  createAnonymousSentinelActor,
  getPermissions,
  listProposals,
  listUnresolvedFundingOperations,
  loadPublicTelemetry,
  parseNat,
  parseNat32,
  parsePrincipal,
  sentinelManagement,
  type SentinelActor,
} from './cycleSentinelService';

const overview = {} as PublicOverview;
type MockActor = SentinelActor & Record<string, ReturnType<typeof vi.fn>>;
const actorOf = (value: Record<string, unknown>): MockActor => value as MockActor;

describe('cycleSentinelService', () => {
  it('loads public telemetry through the injected anonymous actor and paginates public rows', async () => {
    const actor = actorOf({
      get_public_overview: vi.fn().mockResolvedValue(overview),
      list_public_targets: vi.fn()
        .mockResolvedValueOnce({ Ok: { items: [], next_cursor: ['target-next'] } })
        .mockResolvedValueOnce({ Ok: { items: [], next_cursor: [] } }),
      list_public_alarms: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: [] } }),
    });
    const result = await loadPublicTelemetry(actor);
    expect(result.overview).toBe(overview);
    expect(actor.list_public_targets).toHaveBeenNthCalledWith(2, ['target-next'], 100);
  });

  it('fails closed when no Sentinel canister is configured and never accepts aaaaa-aa', () => {
    expect(() => createAnonymousSentinelActor('')).toThrow('not configured');
    expect(() => createAnonymousSentinelActor('aaaaa-aa')).toThrow('not configured');
  });

  it('defends every paginated authenticated query against a repeated cursor', async () => {
    const actor = actorOf({
      list_governance_proposals: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: ['same'] } }),
    });
    await expect(listProposals(actor)).rejects.toThrow('repeated proposal cursor');
    expect(actor.list_governance_proposals).toHaveBeenCalledTimes(2);
  });

  it('paginates governance proposals and unresolved funding operations', async () => {
    const proposal = {} as ProposalRecord;
    const actor = actorOf({
      list_governance_proposals: vi.fn()
        .mockResolvedValueOnce({ Ok: { items: [proposal], next_cursor: ['p2'] } })
        .mockResolvedValueOnce({ Ok: { items: [], next_cursor: [] } }),
      list_unresolved_funding_operations: vi.fn()
        .mockResolvedValueOnce({ Ok: { items: [], next_cursor: ['o2'] } })
        .mockResolvedValueOnce({ Ok: { items: [], next_cursor: [] } }),
    });
    expect(await listProposals(actor)).toEqual([proposal]);
    expect(await listUnresolvedFundingOperations(actor)).toEqual([]);
    expect(actor.list_unresolved_funding_operations).toHaveBeenNthCalledWith(2, ['o2'], 100);
  });

  it('treats only Err.NotSigner as the nonsigner result and preserves other errors', async () => {
    const nonsigner = actorOf({ get_my_permissions: vi.fn().mockResolvedValue({ Err: { NotSigner: null } }) });
    await expect(getPermissions(nonsigner)).resolves.toEqual({ is_signer: false });
    const blocked = actorOf({ get_my_permissions: vi.fn().mockResolvedValue({ Err: { InvalidCursor: null } }) });
    await expect(getPermissions(blocked)).rejects.toThrow('InvalidCursor');
    expect(() => canisterError({ Err: 'backend message' })).toThrow('backend message');
  });

  it('keeps approval and execution separate and preserves bigint IDs', async () => {
    const actor = actorOf({
      approve_proposal: vi.fn().mockResolvedValue({ Ok: true }),
      execute_proposal: vi.fn().mockResolvedValue({ Ok: null }),
      resolve_unknown_as_spent: vi.fn().mockResolvedValue({ Ok: {} }),
    });
    const id = 999999999999999999999999n;
    await sentinelManagement.approveProposal(actor, id);
    expect(actor.approve_proposal).toHaveBeenCalledWith(id);
    expect(actor.execute_proposal).not.toHaveBeenCalled();
    await sentinelManagement.executeProposal(actor, id);
    await sentinelManagement.resolveUnknownAsSpent(actor, id);
    expect(actor.execute_proposal).toHaveBeenCalledWith(id);
    expect(actor.resolve_unknown_as_spent).toHaveBeenCalledWith(id);
  });

  it('validates principals, text, bigint nat fields, nat32, and management errors before calls', async () => {
    expect(() => parsePrincipal('', 'Target principal')).toThrow('required');
    expect(() => parsePrincipal('aaaaa-aa', 'Target principal')).toThrow('aaaaa-aa');
    expect(() => parseNat('-1', 'Operation ID')).toThrow('non-negative');
    expect(parseNat('900719925474099312345', 'Operation ID')).toBe(900719925474099312345n);
    expect(() => parseNat32('4294967296', 'Signer threshold')).toThrow('nat32');
    const actor = actorOf({ propose_set_signer_threshold: vi.fn().mockResolvedValue({ Ok: 1n }) });
    expect(() => sentinelManagement.proposeSetSignerThreshold(actor, 0)).toThrow('positive nat32');
    expect(actor.propose_set_signer_threshold).not.toHaveBeenCalled();
    const rejected = actorOf({ propose_set_signer_threshold: vi.fn().mockResolvedValue({ Err: { ThresholdZero: null } }) });
    await expect(sentinelManagement.proposeSetSignerThreshold(rejected, 1)).rejects.toThrow('ThresholdZero');
  });
});
