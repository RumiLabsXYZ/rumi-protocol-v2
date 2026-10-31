# BB-31 EVM official-source facts (six EVM destinations)

Card BB-31 | Worker W11 | Date: 2026-10-30 | Scope: official-source facts only; nothing here deploys or activates anything.

Rules used: every fact below was read on 2026-10-30 from the cited URL (the "Verified" column is 2026-10-30 for every row). Primary = chain team docs, DFINITY docs, wallet vendor docs, EIPs. Rows marked [secondary] come from non-primary pages and must not be treated as confirmed. "(derived)" means arithmetic on primary figures. "(repo)" means a file in this repository, compared for reuse only. No Rumi addresses, keys or balances are recorded. Where a primary source did not say, the cell reads "not found in a primary source".

## 0. ICP-side facts common to all six chains

| Fact | Value | Primary source URL | Verified |
|---|---|---|---|
| EVM RPC canister (mainnet) | `7hfb6-caaaa-aaaar-qadga-cai`, 34-node fiduciary subnet; do not deploy your own on mainnet | https://skills.internetcomputer.org/.well-known/skills/evm-rpc/SKILL.md | 2026-10-30 |
| Native chain variants | Only `EthMainnet`, `EthSepolia`, `ArbitrumOne`, `BaseMainnet`, `OptimismMainnet`, plus `Custom { chainId, services: vec { url, headers } }` | https://raw.githubusercontent.com/dfinity/evm-rpc-canister/main/candid/evm_rpc.did | 2026-10-30 |
| Built-in providers | Eth mainnet: Alchemy, Ankr, BlockPi, Cloudflare, PublicNode, Llama. Sepolia: Alchemy, Ankr, BlockPi, PublicNode, Sepolia. L2 (Arbitrum, Base, OP): Alchemy, Ankr, BlockPi, PublicNode, Llama. (The skill table says Ankr has no Sepolia; the candid lists it. Trust the candid.) | candid URL above; skill URL above | 2026-10-30 |
| Block tags | candid `BlockTag` has `Safe`, `Finalized`, `Latest`, `Number`, `Pending`, `Earliest`. The skill gives no guidance on `finalized` or `safe` trade-offs | candid URL above | 2026-10-30 |
| Consensus | Default `Equality` fails for `Latest` reads; recommended `Threshold { total: Some(3), min: 2 }`; handle `Consistent` and `Inconsistent` or the canister traps | https://skills.internetcomputer.org/.well-known/skills/evm-rpc/SKILL.md | 2026-10-30 |
| Cycle cost | `(5_912_000 + 60_000*nodes + 2400*req_bytes + 800*max_resp_bytes) * nodes * rpc_count`, nodes = 34; start with 10B cycles, unused refunded | same skill | 2026-10-30 |
| Custom provider path | `Custom` carries a chain ID and a list of `{url, headers}`. DFINITY pages do not explain API-key handling. (repo) the Base bridge accepts `Custom { url }` with no headers on purpose so secrets are never in canister state | candid URL above; (repo) `src/rumi_icusd_bridge/src/types.rs` | 2026-10-30 |
| HTTPS outcalls (generic) | HTTPS only, public endpoints only; replicated mode needs a deterministic transform (query fn, strip headers); max response 2,000,000 bytes incl. headers; 30 s remote timeout; replicated POSTs are sent by every node (use idempotency keys or non-replicated mode) | https://skills.internetcomputer.org/.well-known/skills/https-outcalls/SKILL.md | 2026-10-30 |
| IPv6 | The https-outcalls skill and docs state no IPv6 requirement. DFINITY forum announcement: "Canisters can now make HTTPS outcalls to services hosted on both IPv6 and IPv4 addresses." (post date not shown; a reply dated 2025-08-08 says it is live; IPv4 pricing and limits in the full post were not readable) | https://forum.dfinity.org/t/http-outcall-improvements/52419 ; https://docs.internetcomputer.org/building-apps/network-features/using-http/https-outcalls/overview | 2026-10-30 |
| Threshold ECDSA | `sign_with_ecdsa` on curve `secp256k1`; `key_1` (production, 34-node fiduciary subnet, 26,153,846,153 cycles per signature) and `test_key_1` (13-node, 10,000,000,000 cycles); `ecdsa_public_key` free | https://docs.internetcomputer.org/references/cycle-costs/ | 2026-10-30 |
| EIP-155 signing | `v = {0,1} + CHAIN_ID*2 + 35`; secp256k1 output is directly usable for EIP-155 and typed (type 2) envelopes | https://eips.ethereum.org/EIPS/eip-155 | 2026-10-30 |
| Existing Base design (repo) | Settles on `eth_getBlockByNumber("finalized")` with a provider quorum; chain-ID allowlist {8453, 84532, 31337} and a `BaseMainnet` provider variant that is rejected for any other chain (`src/rumi_icusd_bridge/src/state.rs`, `types.rs`, `evm/rpc.rs`); contract `foundry/src/BridgedIcUSD.sol` (solc 0.8.24, OpenZeppelin ERC20 + AccessControlDefaultAdminRules, 8 decimals, uses `block.timestamp` only, no `block.number`) | (repo) | 2026-10-30 |
| Deploy gas on Base (repo) | Packet estimates about 2,795,263 gas to deploy the contract | (repo) `docs/coordination/base-icusd-bridge-sepolia-packet.md` | 2026-10-30 |

Note on "unchanged" below: the contract bytecode is chain-agnostic (chain ID enters only through the canister-computed `mintId` and derivation path). The strings `VERSION = "rumi-base-icusd-bridge-v1"` and the `rumi.icusd.base.bridge-out.v1` domain are Base-labelled; a per-chain relabel is a cosmetic contract edit that off-chain decoders pin, so treat it as a config-level decision, not new logic.

## 1. Ethereum

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Mainnet | Ethereum mainnet, chain ID 1, live; gas token ETH | https://eips.ethereum.org/EIPS/eip-155 ; https://ethereum.org/en/developers/docs/networks/ | 2026-10-30 |
| 1 | Testnets | Sepolia 11155111 (recommended app-dev testnet); Hoodi 560048 (validator/staking testing, open validator set); Holesky shut down September 2025 | https://github.com/eth-clients/sepolia ; https://ethereum.org/en/developers/docs/networks/ ; Hoodi ID: https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 2 | Token standard | Native ERC-20. Contract code limit 24,576 bytes. No chain constraint on decimals or deployment found in a primary source; plain OZ single-minter ERC-20 deploys unchanged | https://eips.ethereum.org/EIPS/eip-170 | 2026-10-30 |
| 3 | Finality | Gasper: checkpoint justified at 2/3 stake, finalized by a following justified checkpoint. Slot 12 s, epoch 32 slots = 384 s (derived). Finalized is about 2 epochs, about 13 minutes (Arbitrum and Robinhood docs both state ~13 min). Reverting a finalized block needs at least 1/3 of stake destroyed | https://ethereum.org/en/developers/docs/consensus-mechanisms/pos/gasper/ ; https://ethereum.org/en/developers/docs/consensus-mechanisms/pos/ ; https://docs.arbitrum.io/how-arbitrum-works/reference/finality-and-reorgs | 2026-10-30 |
| 3 | Bridge wait | Use the `finalized` tag (json-rpc docs define `safe` and `finalized` heads) | https://ethereum.org/en/developers/docs/apis/json-rpc/ | 2026-10-30 |
| 4 | EVM RPC canister | Native: `EthMainnet`, `EthSepolia`. Hoodi would need `Custom` | candid URL in section 0 | 2026-10-30 |
| 4 | Public endpoints | Not found in a primary source (ethereum.org publishes none); use built-in providers | https://ethereum.org/en/developers/docs/networks/ | 2026-10-30 |
| 5 | Signing | EIP-155 and EIP-1559 type 2 (base fee burned, `maxFeePerGas` above base fee plus tip, difference refunded). No chain quirks found in the pages read | https://eips.ethereum.org/EIPS/eip-155 ; https://ethereum.org/en/developers/docs/gas/ | 2026-10-30 |
| 6 | Wallets | MetaMask preconfigured for Ethereum, Sepolia and Hoodi. Rabby: not found in a primary source | https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 7 | Prerequisites | ETH for the minter and admin addresses and the deployer; contract deployment; no allowlist exists. Conflicts: no Ethereum icUSD token is known to this repo (not checked on-chain). Design note (not a source fact): Ethereum is the settlement layer for the OP and Arbitrum standard bridges, so an Ethereum icUSD token later bridged by a canonical bridge could create a second L2 representation | https://docs.base.org/base-chain/network-information/base-contracts (lists the standard bridge and `OptimismMintableERC20Factory`) | 2026-10-30 |
| 8 | Reuse verdict | Contract: unchanged. Canister: needs code change (add 1 and 11155111 to the chain allowlist; add `EthMainnet` and `EthSepolia` provider variants, or use `Custom`) | (repo) `src/rumi_icusd_bridge/src/state.rs` | 2026-10-30 |

## 2. Base (reference design)

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Network | Base Mainnet 8453, live; Base Sepolia 84532 (official testnet); gas ETH. Public RPC https://mainnet.base.org and https://sepolia.base.org; separate tx-submission endpoints https://mainnet-sequencer.base.org and https://sepolia-sequencer.base.org (use only for sending) | https://docs.base.org/base-chain/quickstart/connecting-to-base | 2026-10-30 |
| 2 | Token standard | ERC-20 unchanged (this is the reference design). Base also ships an optional native token standard B20 (ERC-20-compatible superset, precompile-based, Beryl upgrade); not needed here. Standard bridge and `OptimismMintableERC20Factory` exist on Base | https://docs.base.org/upgrades/beryl/b20 [from search summary] ; https://docs.base.org/base-chain/network-information/base-contracts | 2026-10-30 |
| 3 | Finality | Flashblock about 200 ms; L2 block about 2 s; L1 batch inclusion about 2 min; L1 batch finality about 20 min (batch older than 2 epochs / 64 L1 blocks). Only one Base L2 block has ever reorged; an L1 reorg usually does not reorg Base. Base page does not mention `safe`/`finalized` tags; OP Stack docs map RPC `safe` to batch posted on L1 and `finalized` to the Ethereum-finalized L1 block (15 to 30 min on OP Mainnet) | https://docs.base.org/base-chain/network-information/transaction-finality ; https://docs.optimism.io/op-stack/transactions/transaction-finality [from search summary] | 2026-10-30 |
| 3 | Bridge wait | `finalized` (as the existing canister does) | (repo) `src/rumi_icusd_bridge/src/evm/rpc.rs` | 2026-10-30 |
| 4 | EVM RPC canister | Native `BaseMainnet`. Base Sepolia is not native: `Custom` | candid URL in section 0 | 2026-10-30 |
| 5 | Signing | EIP-155 and type 2 work (existing Sepolia lane). Docs list a new Base transaction type for batching and sponsorship, not required here. L1 data fee component: the repo packet budgets for it; a Base-docs statement was not fetched this run | https://docs.base.org/base-chain/network-information/diffs-ethereum-base (index only) ; (repo) `foundry/DEPLOY-BASE.md` | 2026-10-30 |
| 6 | Wallets | MetaMask preconfigured for Base 8453 and Base Sepolia 84532. Rabby: not found in a primary source | https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 7 | Prerequisites | ETH on Base for minter, admin and deployer; deploy contract; no allowlist found. Conflicts: standard bridge exists but no L1 icUSD does | https://docs.base.org/base-chain/network-information/base-contracts | 2026-10-30 |
| 8 | Reuse verdict | Unchanged (it is the reference; a mainnet 8453 install is config only) | (repo) | 2026-10-30 |

## 3. Arbitrum (Arbitrum One; Nova out of scope)

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Network | Arbitrum One 42161 (Nitro rollup, live); Arbitrum Sepolia 421614 (official testnet, dispute window about 4 min); Nova 42170 is AnyTrust and not targeted (Alchemy, Nova Arbiscan and Tenderly dropped Nova on 2026-01-31). Gas token: ETH (page does not state it outright; bonds are in ETH) | https://docs.arbitrum.io/for-devs/dev-tools-and-resources/chain-info | 2026-10-30 |
| 1 | Public RPC | https://arb1.arbitrum.io/rpc and https://sepolia-rollup.arbitrum.io/rpc. Docs state these public endpoints do not support WebSocket or IPv6 | same page | 2026-10-30 |
| 2 | Token standard | Solidity deploys as on Ethereum "with only a few minor functional differences"; all Ethereum precompiles plus Arbitrum ones; PUSH0 supported since ArbOS 11; `block.number` returns an L1 estimate (the contract does not use it). Contract size limit and EVM version not stated on the pages fetched | https://docs.arbitrum.io/build-decentralized-apps/arbitrum-vs-ethereum/comparison-overview ; https://docs.arbitrum.io/build-decentralized-apps/arbitrum-vs-ethereum/solidity-support | 2026-10-30 |
| 3 | Finality | `latest` = sequencer soft confirmation (sub-second, reversible); `safe` = batch posted and in Ethereum `safe` block (about 1 epoch); `finalized` = batch in Ethereum `finalized` block (about 2 epochs, about 13 min). Rule: "An Arbitrum block is only as final as the parent-chain block that carries its batch." Guidance: wait for `finalized` before irreversible actions; wait on the tag, not a block count; store block hashes and handle rewinds | https://docs.arbitrum.io/how-arbitrum-works/reference/finality-and-reorgs | 2026-10-30 |
| 3 | Not needed | 45,818-block (about 6.4 day) dispute window applies to L2-to-L1 withdrawals via the outbox, not to a burn-and-release bridge | https://docs.arbitrum.io/how-arbitrum-works/deep-dives/l2-to-l1-messaging | 2026-10-30 |
| 4 | EVM RPC canister | Native `ArbitrumOne` (Alchemy, Ankr, BlockPi, PublicNode, Llama). Arbitrum Sepolia is not native: `Custom` | candid URL in section 0 | 2026-10-30 |
| 5 | Signing and gas | Total fee = child basefee x (child gas + parent calldata charge); "poster fee" is computed from Brotli-compressed tx size x 16; `eth_estimateGas` includes it and moves with L1 data price, so estimate close to send time; receipts carry `gasUsedForL1`. Gas price floor 0.02 gwei (One), 0.2 gwei (Sepolia). Tips ignored unless the chain owner enables them | https://docs.arbitrum.io/how-arbitrum-works/gas-fees ; chain-info page above | 2026-10-30 |
| 6 | Wallets | MetaMask preconfigured for Arbitrum 42161 and Arbitrum Sepolia 421614. Rabby: not found in a primary source | https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 7 | Prerequisites | ETH on Arbitrum One for minter, admin, deployer; deploy contract; no allowlist found. Conflicts: Arbitrum token bridge exists for L1 tokens; none for icUSD | https://docs.arbitrum.io/for-devs/dev-tools-and-resources/chain-info | 2026-10-30 |
| 8 | Reuse verdict | Contract unchanged. Canister needs code change: allowlist 42161/421614 and the `ArbitrumOne` provider variant (or `Custom`); gas buffers must cover the L1 poster fee | (repo) `src/rumi_icusd_bridge/src/state.rs` | 2026-10-30 |

## 4. Robinhood Chain (Robinhood's own Arbitrum-based L2; not brokerage crypto support)

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Network | Mainnet chain ID 4663, "Robinhood Chain is live"; testnet 46630; gas token ETH; Arbitrum dedicated chain settling on Ethereum, Ethereum blobs for data availability. Launch date not in the primary pages fetched ([secondary] reports say 2026-07-01) | https://docs.robinhood.com/chain/ ; https://docs.robinhood.com/chain/connecting | 2026-10-30 |
| 1 | Explorers | Mainnet robinhoodchain.blockscout.com; testnet explorer.testnet.chain.robinhood.com | https://docs.robinhood.com/chain/connecting | 2026-10-30 |
| 1 | Public RPC | Mainnet https://rpc.mainnet.chain.robinhood.com ; testnet https://rpc.testnet.chain.robinhood.com . Docs: free public endpoints are rate-limited and not recommended for production. Recommended provider Alchemy (needs API key); others: Chainstack, QuickNode, Blockdaemon, dRPC, Validation Cloud, GlobalStake, Pinax | https://docs.robinhood.com/chain/add-network-to-wallet/ ; https://docs.robinhood.com/chain/connecting | 2026-10-30 |
| 2 | Token standard | Solidity and Vyper contracts "deploy without modification" with standard tooling; max code size 96 KB, init code 192 KB; permissioned deployment: not stated anywhere fetched (no allowlist mentioned). Stock Tokens are a separate Robinhood section | https://docs.robinhood.com/chain/differences-from-ethereum/ ; https://docs.robinhood.com/chain/deploy-smart-contracts/ | 2026-10-30 |
| 3 | Finality | Soft confirmation (sub-second, reversible only if the sequencer posts a different order) then posted to Ethereum Inbox (minutes) then Ethereum finality about 13 min after posting. Guidance: for high-value or irreversible actions wait for posting or full finality. `safe`/`finalized` tags not mentioned. Arbitrum docs warn tags on dedicated chains need parent-chain finality data exposed by the node, so test the tag before relying on it. 7-day challenge period is for canonical withdrawals only | https://docs.robinhood.com/chain/transaction-finality/ ; https://docs.arbitrum.io/how-arbitrum-works/reference/finality-and-reorgs | 2026-10-30 |
| 4 | EVM RPC canister | Not native: `Custom { chainId: 4663 }`. Only one keyless public endpoint found, so a 2-of-3 quorum needs extra providers; keyed Alchemy URLs conflict with the no-secrets-in-state rule unless headers/keys are handled | https://docs.robinhood.com/chain/connecting ; candid URL in section 0 | 2026-10-30 |
| 5 | Signing and gas | Two-part fee (L2 execution plus L1 data fee), `eth_estimateGas` covers both; ArbGasInfo and NodeInterface precompiles; ordering first-come first-served; `block.number` is an L1 estimate; EIP-1559 and tx types not stated | https://docs.robinhood.com/chain/gas-and-fees/ ; https://docs.robinhood.com/chain/differences-from-ethereum/ | 2026-10-30 |
| 6 | Wallets | MetaMask preconfigured (4663 and 46630) and its docs list the chain; Robinhood Wallet native; Robinhood's add-network page documents manual MetaMask steps and does not mention Rabby. Rabby support only in [secondary] news | https://docs.metamask.io/agent-wallet/reference/supported-chains/ ; https://docs.robinhood.com/chain/add-network-to-wallet/ | 2026-10-30 |
| 7 | Prerequisites | ETH on Robinhood Chain for gas addresses; deploy; no allowlist found. Conflicts: canonical Arbitrum bridge; routes (LayerZero/Stargate, CCIP, Relay, Across) for ETH, USDG and others; no icUSD conflict | https://docs.robinhood.com/chain/bridging/ | 2026-10-30 |
| 8 | Reuse verdict | Contract unchanged (confirm PUSH0 at the installed ArbOS: Robinhood docs do not state the ArbOS version). Canister needs code change: allowlist 4663/46630, `Custom` provider path with at least 2 independent providers, tag-support probe | https://docs.robinhood.com/chain/protocol-contracts/ (no ArbOS version) | 2026-10-30 |

## 5. HyperEVM (EVM layer of Hyperliquid; distinct from HyperCore)

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Network | Mainnet chain ID 999, testnet 998; gas token HYPE (18 decimals); no explicit launch statement fetched, but docs describe a live mainnet RPC | https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm | 2026-10-30 |
| 1 | Public RPC | Mainnet https://rpc.hyperliquid.xyz/evm (no websocket); testnet https://rpc.hyperliquid-testnet.xyz/evm . Only this one official mainnet endpoint found | same page | 2026-10-30 |
| 2 | Token standard | EVM at Cancun hardfork without blob support; EIP-1559 enabled, base and priority fees burned. Block gas limits: small block 3M gas every 1 s, big block 30M gas every 1 min. Contract size limit: not found in a primary source. The repo contract's estimated deploy gas (about 2.8M, on Base) sits under 3M but with thin margin; if deploy needs a big block the sender must be an existing HyperCore user and set `usingBigBlocks` (HyperCore action `evmUserModify`) | https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm/dual-block-architecture ; (repo) packet | 2026-10-30 |
| 2 | Optional | Linking an ERC-20 to a HyperCore spot token uses `requestEvmContract` and `finalizeEvmContract`; not needed for a bridge token | https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm/hypercore-less-than-greater-than-hyperevm-transfers | 2026-10-30 |
| 3 | Finality | Consensus HyperBFT (HotStuff variant); the EVM "gets its security from HyperBFT". Docs do not state EVM finality semantics, reorg behaviour or `safe`/`finalized` tag support; they say `eth_call`, `eth_estimateGas`, `eth_getBalance`, `eth_getCode`, `eth_getStorageAt`, `eth_getTransactionCount` support only the latest block. [secondary] reports say committed blocks are final | https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm ; https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm/json-rpc | 2026-10-30 |
| 4 | EVM RPC canister | Not native: `Custom { chainId: 999 }`. `eth_getLogs` limited to 4 topics and 50 blocks; no historical state on the default RPC; rate limits "same as the API server" (no numbers); `eth_maxPriorityFeePerGas` always 0; `eth_syncing` always false; extras `eth_bigBlockGasPrice`, `eth_usingBigBlocks` | https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/hyperevm/json-rpc | 2026-10-30 |
| 5 | Signing and gas | EIP-1559 enabled (type 2 ok); `eth_gasPrice` is the next small-block base fee, big blocks use `eth_bigBlockGasPrice`; two independent mempools; each address may queue only the next 8 nonces; txs older than 1 day pruned | dual-block page and json-rpc page above | 2026-10-30 |
| 6 | Wallets | MetaMask preconfigured for HyperEVM 999 and 998. Rabby: not found in a primary source ([secondary] user reviews only) | https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 7 | Prerequisites | HYPE for minter, admin and deployer addresses (a plain EOA must first receive HYPE on HyperEVM); deploy; HyperCore user only if big blocks are needed. Existing official USD tokens on HyperEVM: not checked this run | dual-block page above | 2026-10-30 |
| 8 | Reuse verdict | Contract unchanged if it deploys in a small block (else big-block step). Canister needs code change: allowlist 999/998, `Custom` providers (only one official endpoint, need at least 2 independent), `finalized` tag handling, 8-nonce queue limit | (repo) `src/rumi_icusd_bridge/src/evm/rpc.rs` | 2026-10-30 |

## 6. Monad

| # | Fact | Value | Primary source URL | Verified |
|---|---|---|---|---|
| 1 | Network | Mainnet chain ID 143, gas MON, live (docs show current version v0.16.4, revision MONAD_TEN). Launch 2025-11-24 is [secondary] only. Testnet chain ID 10143, MON, reset from genesis 2025-12-16, version v0.16.5, faucet https://faucet.monad.xyz | https://docs.monad.xyz/developer-essentials/network-information ; https://docs.monad.xyz/developer-essentials/testnets | 2026-10-30 |
| 1 | Public RPC | Mainnet: rpc.monad.xyz (QuickNode, 25 rps), rpc1.monad.xyz (Alchemy, 15 rps), rpc2.monad.xyz (Goldsky, 300/10 s), rpc3.monad.xyz (Ankr, 300/10 s), rpc-mainnet.monadinfra.com (Monad Foundation, 20 rps, historical state). Testnet: https://testnet-rpc.monad.xyz , https://rpc.ankr.com/monad_testnet , https://rpc-testnet.monadinfra.com . Rate-limited, "enough for basic usage" | same two pages | 2026-10-30 |
| 2 | Token standard | Max contract code 128 KB, init code 256 KB (Ethereum 24 KB / 48 KB); all Fusaka opcodes; tx types 0, 1, 2 and 4 (EIP-7702) supported, type 3 (blob) not; some opcodes repriced. A plain OZ ERC-20 fits and deploys unchanged. No deployment allowlist found | https://docs.monad.xyz/developer-essentials/summary ; https://docs.monad.xyz/developer-essentials/differences | 2026-10-30 |
| 3 | Finality | MonadBFT: block time 300 ms; speculative finality in 1 slot (Voted, about 300 ms); full finality in 2 slots (Finalized, about 600 ms); Verified (state root) 3 blocks after Finalized. RPC tags: `latest` = Proposed, `safe` = Voted, `finalized` = Finalized (not revertible without a hard fork); `latest` can return non-finalized data | https://docs.monad.xyz/monad-arch/consensus/monad-bft ; https://docs.monad.xyz/developer-essentials/summary ; https://docs.monad.xyz/monad-arch/consensus/block-states ; https://docs.monad.xyz/reference/json-rpc/overview [from search summary] | 2026-10-30 |
| 3 | Bridge wait | `finalized` (about 600 ms, fastest of the six) | block-states page above | 2026-10-30 |
| 4 | EVM RPC canister | Not native: `Custom { chainId: 143 }`. Five official public endpoints from different operators, so a 2-of-3 quorum is feasible without keys. `eth_getLogs` block-range limits are provider-specific (100 to 1,000 blocks; error -32602 above) | network-information page above; https://docs.monad.xyz/reference/json-rpc/overview [from search summary] | 2026-10-30 |
| 5 | Signing and gas | EIP-1559 pricing; the sender pays `gas_limit x price`, not gas used, so set explicit limits and never accept wallet "very high default" limits; base fee minimum 100 MON-gwei; tx gas limit 30M, block 150M. Reserve balance: 10 MON per EOA; a tx that dips below it can be included and still revert while paying gas; an "emptying exception" applies to a sender's only tx in the last 3 blocks; no global mempool (txs forwarded to the next few leaders) | https://docs.monad.xyz/developer-essentials/gas-pricing ; https://docs.monad.xyz/developer-essentials/reserve-balance ; https://docs.monad.xyz/developer-essentials/differences | 2026-10-30 |
| 6 | Wallets | Monad docs list 17 mainnet wallets including MetaMask and Rabby (both mainnet and testnet); MetaMask docs list Monad 143 and 10143 | https://docs.monad.xyz/tooling-and-infra/wallets/software-wallets ; https://docs.metamask.io/agent-wallet/reference/supported-chains/ | 2026-10-30 |
| 7 | Prerequisites | MON for gas addresses, kept well above the 10 MON reserve so back-to-back txs do not revert; deploy; no allowlist found. Conflict (repo): `foundry/src/IcUSD.sol` is a separate debt-backed icUSD for Monad and Conflux; confirm whether one is deployed on Monad mainnet before adding an escrow-backed token with the same symbol | (repo) natspec in `foundry/src/BridgedIcUSD.sol` | 2026-10-30 |
| 8 | Reuse verdict | Contract unchanged. Canister needs code change: allowlist 143/10143, `Custom` providers, explicit gas limits (not estimate-derived), reserve-aware funding checks, log range chunking | (repo) `src/rumi_icusd_bridge/src/evm` | 2026-10-30 |

## 7. Comparison across the six chains

| Chain | Chain ID mainnet / testnet | Gas | Finality wait (bridge) | Time to wait | EVM RPC canister | Reuse verdict |
|---|---|---|---|---|---|---|
| Ethereum | 1 / 11155111 (Sepolia) | ETH | `finalized` | about 13 min | native (`EthMainnet`, `EthSepolia`) | contract unchanged; canister code change (allowlist, variants) |
| Base | 8453 / 84532 | ETH | `finalized` (L1 batch finality) | about 20 min | native mainnet; Sepolia via `Custom` | unchanged (reference) |
| Arbitrum One | 42161 / 421614 | ETH | `finalized` (parent finality) | about 13 min after batch posting | native mainnet; Sepolia via `Custom` | contract unchanged; canister code change (allowlist, variant, L1 fee buffer) |
| Robinhood Chain | 4663 / 46630 | ETH | Ethereum finality of the posted batch; tag support unverified | minutes to post, then about 13 min | `Custom` only | contract unchanged; canister code change (allowlist, 2+ providers, tag probe) |
| HyperEVM | 999 / 998 | HYPE | HyperBFT commit; `finalized` tag unverified | about 1 s blocks (small), 1 min (big) | `Custom` only | contract unchanged if deploy fits 3M gas small block; canister code change |
| Monad | 143 / 10143 | MON | `finalized` (MonadBFT) | about 600 ms | `Custom` only | contract unchanged; canister code change (gas limits, reserve, log ranges) |

## 8. Open questions

1. Robinhood Chain and HyperEVM: do the public RPCs return a real `finalized` block for `eth_getBlockByNumber("finalized")`? Neither primary page documents `safe` or `finalized`. A read-only probe of the public RPCs is the cheapest check (not run here; this card is web research only).
2. HyperEVM: does a primary source state finality and reorg behaviour for EVM blocks? Only "secured by HyperBFT" was found. Also confirm real deploy gas on HyperEVM against the 3M small-block limit (the 2.8M figure is a Base estimate).
3. Quorum on single-endpoint chains: HyperEVM and Robinhood publish one official public endpoint each. Which third-party providers are acceptable, and how are API keys kept out of canister state (the current `Custom { url }` has no headers)?
4. IPv4 outcalls: the DFINITY forum announcement says IPv4 and IPv6 are both supported, but its pricing and limits were not readable. Confirm cost impact before choosing endpoints (Arbitrum public RPC is documented as IPv4-only).
5. Rabby: no primary source found for Ethereum, Base, Arbitrum, Robinhood or HyperEVM (Monad lists it). MetaMask's own docs list all six mainnets and testnets.
6. Base: the docs mention a new Base transaction type and a native B20 token standard. Confirm type 2 and plain ERC-20 remain fully supported on Base mainnet (the Sepolia design already works).
7. Monad: confirm whether a debt-backed `IcUSD` already exists on Monad mainnet (token naming and user confusion). Also confirm Monad's `eth_getLogs` range limit matches observer chunk sizes.
8. Arbitrum One and Robinhood: contract size limit is stated only for Robinhood (96 KB); not an issue for this contract either way. Robinhood ArbOS version (PUSH0) not stated.
9. Block time and launch-date facts marked [secondary] (Monad launch 2025-11-24, Robinhood launch 2026-07-01) are not needed for design; do not cite them publicly without a primary source.

## Coordinator addendum: activation constraints from focused source checks (2026-10-30)

Added by the coordinator after Miles' scoped review of this card. These are explicit DISABLED
blockers for the affected profiles, not configuration choices. Groundwork records them; nothing here
weakens a read.

What the Base bridge observer reads, and at which block (src/rumi_icusd_bridge/src/observer.rs):
- Bind (around lines 255-268): the contract must be pristine at a finalized block.
- Bridge-out scan (around lines 612-648): `bridgeOutCount` and `totalMinted` are read with `eth_call`
  at the numbered finalized head, and receipts and logs are checked against it.
- Supply audit (around lines 905-922): `totalSupply`, `bridgeOutCount` and `totalMinted` are read at
  the numbered finalized head returned by `rpc::finalized_head()`.

So every destination must serve numbered, finalized, historical contract state from at least the
quorum of independent providers the bridge requires. Reads must never fall back to `latest`.

| Chain | Constraint | Source | Activation prerequisite (profile stays disabled until met) |
| --- | --- | --- | --- |
| HyperEVM | The official default JSON-RPC serves `eth_call` against `latest` only, not a numbered historical block (Miles read the official HyperEVM JSON-RPC page; this file's HyperEVM section already lists only one keyless endpoint). | HyperEVM JSON-RPC docs on hyperliquid.gitbook.io | Either archive-capable independent providers that answer numbered finalized `eth_call`, receipts and logs, in the number the quorum needs, or a reviewed observer redesign that does not need historical state. Not config-only. |
| Monad | Monad's block states separate Finalized (ordering fixed; the `finalized` RPC tag) from Verified (execution outputs agreed by a supermajority, at `latest_finalized_block - execution_delay`). The page does not give the delay's value. | https://docs.monad.xyz/monad-arch/consensus/block-states (verified 2026-10-30) | Receipts, logs and the supply audit depend on execution outputs, so the bridge must take them from a Verified block: head = finalized minus the execution delay, with the delay taken from a primary source. Reading at the `finalized` head is ordering-level evidence only. Needs an observer head-selection change per chain. Not config-only. |

Base, Ethereum, Arbitrum and Robinhood Chain keep the existing finalized-head model, subject to this
file's open questions (for example whether Robinhood Chain's public RPC serves a real `finalized` tag).

Coordinator reconciliation (2026-10-30, after BB-34): the Monad execution delay IS sourced. Monad's developer summary
(https://docs.monad.xyz/developer-essentials/summary, verified 2026-10-30) says Verified ("state root finality") comes
three blocks after Finalized; the block-states page gives only the formula `latest_finalized_block - execution_delay`.
So the Verified head is finalized minus 3 blocks. The `monad-verified-head` blocker stays, because the observer's head
selection still needs a code change; only the "value unsourced" part is resolved.
