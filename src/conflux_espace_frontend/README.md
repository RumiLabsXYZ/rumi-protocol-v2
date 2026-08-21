# Conflux eSpace canary UI

This standalone UI defaults to the existing chain-71 staging configuration:

```sh
npm ci
npm test
npm run check
npm run build
```

The production canary is an explicit local/private build mode. It is not wired
into an ICP deployment recipe:

```sh
npm run build:production-canary
npm run dev:production-canary
```

The production-canary build pins chain 1030, `https://evm.confluxrpc.com`,
`https://evm.confluxscan.io`, backend `tfesu-vyaaa-aaaap-qrd7a-cai`, and IcUSD
`0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff`. It accepts injected wallets only,
declares exactly 5 CFX collateral and 0.10 icUSD debt in the Open intent, and
removes borrow/withdraw controls. Every signature or EVM transaction still
requires an explicit click and wallet confirmation. Building does not deploy or
call production.

The browser persists the canary vault id, lifecycle phase, and submitted
transaction hashes per wallet. It writes a cross-tab safety lock before every
wallet request, so Open, deposit, burn, and close remain locked across reloads
and ambiguous provider responses. Confirmed wallet rejection safely restores
the prior phase. If a prompt is interrupted without a definitive result, the UI
first rechecks backend state and requires an explicit operator confirmation of
no authorization before it can clear that lock. Confirmed reverted or cancelled
EVM transactions expose an explicit retry. A semantic replacement remains
fail-closed because it may have moved funds; it advances only through backend
observation or the same explicit no-action recovery confirmation. Repricing
keeps the action locked under the replacement hash, and transient receipt errors
retry read-only polling. Any existing untracked vault is read-only, and either a
persisted lifecycle or any existing chain-1030 vault permanently disables a
second Open in this build.
