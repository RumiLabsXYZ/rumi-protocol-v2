import { describe, expect, it } from 'vitest';
import {
  DEFAULT_LOCAL_IC_HOST,
  DEFAULT_LOCAL_II_CANISTER_ID,
  localIiUrl,
  localReplicaPort,
  resolveLocalIcHost,
  resolveLocalIiCanisterId,
} from './localNetwork';

describe('localNetwork dev override', () => {
  it('keeps the dfx defaults byte-identical when no env vars are set', () => {
    expect(resolveLocalIcHost({})).toBe('http://localhost:4943');
    expect(resolveLocalIcHost(undefined)).toBe('http://localhost:4943');
    expect(resolveLocalIiCanisterId({})).toBe('uxrrr-q7777-77774-qaaaq-cai');
    expect(localIiUrl(DEFAULT_LOCAL_IC_HOST, DEFAULT_LOCAL_II_CANISTER_ID)).toBe(
      'http://uxrrr-q7777-77774-qaaaq-cai.localhost:4943',
    );
    expect(localReplicaPort(DEFAULT_LOCAL_IC_HOST)).toBe(4943);
  });

  it('treats blank or non-string values as unset', () => {
    expect(resolveLocalIcHost({ VITE_LOCAL_IC_HOST: '   ' })).toBe(DEFAULT_LOCAL_IC_HOST);
    expect(resolveLocalIcHost({ VITE_LOCAL_IC_HOST: 4950 })).toBe(DEFAULT_LOCAL_IC_HOST);
    expect(resolveLocalIiCanisterId({ VITE_LOCAL_II_CANISTER_ID: '' })).toBe(DEFAULT_LOCAL_II_CANISTER_ID);
  });

  it('retargets the gateway, the II subdomain URL and the replica port', () => {
    const host = resolveLocalIcHost({ VITE_LOCAL_IC_HOST: 'http://localhost:4950/' });
    expect(host).toBe('http://localhost:4950');
    const ii = resolveLocalIiCanisterId({ VITE_LOCAL_II_CANISTER_ID: 'rdmx6-jaaaa-aaaaa-aaadq-cai' });
    expect(localIiUrl(host, ii)).toBe('http://rdmx6-jaaaa-aaaaa-aaadq-cai.localhost:4950');
    expect(localReplicaPort(host)).toBe(4950);
  });
});
