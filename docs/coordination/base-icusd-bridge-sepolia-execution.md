# Base Sepolia execution evidence — 2026-10-25

Executor: Miles. This records the isolated test authorized by Rob and described in
`base-icusd-bridge-sepolia-packet.md`, including its dated amendments. Runtime source remains
`4169feb8`; later changes are documentation. This is evidence for the existing BB-24/BB-25 cards,
not another coordination board.

Only the new synthetic test ledger, new bridge canister and new Base Sepolia contract are used.
Existing Rumi canisters, the production icUSD ledger, production frontend, Base mainnet and merge
remain outside this run. Exact targets, transaction receipts, balances and funding records are
kept in the executor's private run record; no key, password or private operational log is included
here.

| Check | Observed proof |
| --- | --- |
| Synthetic ledger install | Installed; live module hash equals packet `cb0c3233ebb137f606d7928e90bfd907fe932a941f11b41c23cf0d6cf9c64802`; TICUSD, eight decimals, seed and fee independently read back |
| Bridge install | Installed; live module hash equals packet `0b324387251dbb8842d3671ab00a70a5b6ccf5fffd2a47727639e11edc6f929a`; sole controller unchanged |
| ICP RPC reachability | Identical custom provider set revalidated through the bridge, chain-id quorum accepted; this alone does not prove every provider or method |
| Base deployment | Successful funding/deployment receipts, matching canonical block hashes and finalized heads on two providers |
| Deployed bytes | Full runtime sha256 equals `0dd0612c45e19fa72ca2dd6b2a4e8234b9964b39f81dcdc5b4a7b865cceb4ec8` on both providers |
| Contract authority/configuration | Live version, decimals, initial supply/counters, caps, windows, delay and pauses checked; threshold minter/admin roles correct; throwaway deployer holds no admin/minter/pauser role |
| Contract binding | Original request reconciled through live stored binding at a finalized block; no repeated bind request; canister checked historical contract reads and limits |
| ICP → Base | One fixed request admitted; synthetic ledger escrow/debit/fee-adjusted supply independently verified; exact mint event and successful receipt finalized on two providers; canister records `Delivered` |
| Mint gas | 149,070 used, below 80% of the unchanged 300,000 limit |
| Same-request recovery | Same client request resubmitted once; returned the same deposit and ledger pull, with no extra ledger debit and zero remaining allowance |
| Base → ICP | One return burn finalized on two providers; exact event amount, sequence and ICP destination verified; canister records `Released`; independent ledger read verifies the expected fee-adjusted credit |
| Return burn gas | 86,530 used under the fixed 200,000 gas envelope |
| Final reconciliation | Explicit fresh reconciliation after release passed; independent ledger/Base balances match the packet, coverage/bound/exact-match flags true, no deficit/surplus and no pending or held operations |
| Close | All four pauses read back true, then test bridge status read back `Stopped`; reviewed module hash and sole controller unchanged; backing and state retained |

The return burn was performed only after the canister recorded `Delivered`, with the mint's
canonical finalized block independently confirmed on both providers. Final acceptance used the
finalized return burn, `Released`, independent ledger/Base balance reads and an explicit fresh
supply audit. A cached audit can predate a transfer; the final check verified that its escrow
snapshot was at or after the release and its finalized Base snapshot was at or after the burn.

Final accounting matched section J's predefined test vectors exactly, including the fee-adjusted
tester balance and remaining Base supply matched by dedicated escrow. Circulating ICP plus
circulating Base equals canonical ledger supply, without counting the locked backing twice.
Pending mint/release/refund, held, reserved pull, unknown outflow and fee buckets are all zero.
The remaining Base tokens are still a liability; stopping does not discharge it. Exact readbacks
are retained privately.

## Reconciliation notes

- The rejected ledger install initialized no token state, but incurred cycle charges. Amendment 1
  preserves the original budget as history and documents the test ledger top-up.
- The original bind exceeded a three-minute local CLI wait. Its result was reconciled by reading
  the stored binding, with the original address and finalized start block, before any further
  action. It was never submitted twice. Binding performs sequential provider reads; a short local
  deadline is not proof that canister execution failed.
- Faucet funding was sufficient under Amendment 2's lower fee/reserve allocation and a fresh
  simulation plus on-chain L1 data-fee upper bound. No ETH purchase or mainnet wallet funding was
  needed.
- The finalized burn was reached through three completed bounded observer passes. Each next
  pass followed a terminal response and observed cursor progress; none created another burn or
  claim. The normal admin tick endpoint advanced work between the long scheduled idle probes.
  Each pass used the configured finalized proofs, providers and accounting.

Proposed catalog lessons (not applied to persona or memory files): quote actual gas/data fees
instead of treating a budget allowance as a funding minimum; reconcile a slow update through
stored state before repeating it; and account for bounded history batches when using a long
test idle interval. These are operational observations from this run, not new production gates.

This record does not establish production readiness, browser-wallet acceptance, or permission to
activate production assets. The source's existing deterministic and independent review evidence
is in `base-icusd-bridge-review-record.md`; public execution is a separate proof level.
