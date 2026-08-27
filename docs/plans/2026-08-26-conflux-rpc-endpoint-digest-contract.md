# Conflux RPC endpoint-set digest operator contract

This is the narrow binding contract between the production backend and a
future Disabled-chain recovery runner. It adds evidence, not authority: this
document and the query below do not authorize an upgrade, recovery call,
endpoint change, deployment, activation, or movement of funds.

## Backend query

`get_chain_rpc_endpoint_set_digest(chain_id)` is a developer-gated, read-only
query method. It returns either:

- `Unauthorized` when the caller is not the configured developer principal;
- `ChainNotRegistered(chain_id)` when no live stored config exists; or
- `ChainRpcEndpointSetDigestV1`, containing only `chain_id`, the exact distinct
  endpoint count, the effective minimum quorum, and a lowercase unprefixed
  64-character SHA-256 digest.

Endpoint URLs never cross this Candid boundary. They may contain provider
credentials. The method uses the same developer-principal authorization
boundary as the chain-admin update methods and performs no state mutation.

## Canonical V1 digest

URL identity is exact UTF-8 byte identity. There is no case, slash, hostname,
percent-encoding, query-string, or credential normalization. This matches the
exact-string de-duplication applied by chain registration and updates.

The SHA-256 preimage is the following byte sequence:

1. the domain `rumi.chain-rpc-endpoint-set-digest.v1`, encoded as its u64
   big-endian byte length followed by its bytes;
2. the chain ID as u32 big-endian;
3. the effective minimum quorum as u32 big-endian (`min_quorum_providers`, or
   the runtime default when absent, clamped by the runtime helper);
4. the distinct endpoint count as u32 big-endian; and
5. every exact distinct endpoint, sorted by UTF-8 bytes, each encoded as its
   u64 big-endian byte length followed by its bytes.

This commits to endpoint identity, chain identity, and effective quorum.
Endpoint order and exact duplicates do not change the digest. Any endpoint or
effective-quorum drift does.

## Recovery-runner binding

Before every permitted recovery mutation, the runner must:

1. start from its sealed, reviewed local endpoint list and expected chain ID
   and effective quorum;
2. calculate this exact V1 digest locally without printing endpoint URLs;
3. call the query as the fixed production developer identity;
4. send it through replicated execution: with `icp canister call`, use the
   default update request and **omit `--query`**; reject any runner mode or
   transcript that used the uncertified fast-query path;
5. require `Ok`, exact chain ID, exact distinct count, exact effective quorum,
   and constant-time-equivalent exact digest equality; and
6. stop before dispatch on `Unauthorized`, `ChainNotRegistered`, decode error,
   malformed digest, or any mismatch.

The sanitized execution transcript may record the method name, chain ID,
count, quorum, digest, query result, source/runner commit hashes, and timestamp.
It must not record raw endpoint URLs. The runner must not accept caller-supplied
URL or quorum overrides in execute mode.

When obtained through replicated execution, the binding proves that the
runner's reviewed set matched the replicated canister state used for that
response. An ordinary `--query` response does not provide this proof and must
never authorize a recovery mutation. The binding does **not** prove provider
ownership independence, provider liveness, historical-state availability,
response agreement, or the absence of a response-to-update race. Those remain
separate recovery gates. A backend upgrade that exposes this method must be
installed and hash-verified before a recovery runner can rely on it; that
upgrade requires its own explicit production approval.
