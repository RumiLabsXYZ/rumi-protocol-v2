# sol-kat

Generates known-answer test (KAT) vectors for the backend's hand-rolled
native-SOL collateral wire format
(`src/rumi_protocol_backend/src/chains/sol`, reusing the pure builders in
`src/rumi_protocol_backend/src/chains/solana/tx.rs`), using the real
`@solana/web3.js` SDK as ground truth. Mirrors `tools/xrp-kat`.

Covers: base58 address encoding, legacy System-transfer message
serialization, durable-nonce transfer message bytes (the exact two-signer
shape `chains::sol::adapter::sign_sol_payment_from` builds), wire-transaction
assembly with two signatures, and compact-u16 (ShortU16) encoding edge cases.

## Regenerate

```bash
cd tools/sol-kat
npm install
node index.mjs > ../../src/rumi_protocol_backend/src/chains/sol/testdata/sol_kat.json
```

The Rust unit tests in
`src/rumi_protocol_backend/src/chains/sol/tests_kat.rs` load this fixture via
`include_str!` and assert byte-for-byte equality against it. If a vector ever
disagrees with a freshly regenerated fixture, the RUST CODE is what is wrong,
never the fixture, per that file's own doc comment.
