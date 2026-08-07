// Generate known-answer test vectors for the backend's hand-rolled native-SOL
// collateral signing/wire path (src/rumi_protocol_backend/src/chains/sol,
// reusing src/rumi_protocol_backend/src/chains/solana/tx.rs's pure builders),
// using the official @solana/web3.js SDK as ground truth.
//
// Regenerate (from the project root):
//   cd tools/sol-kat && npm install && node index.mjs \
//     > ../../src/rumi_protocol_backend/src/chains/sol/testdata/sol_kat.json
//
// The Rust unit tests in chains/sol/tests_kat.rs assert byte-for-byte equality
// against this file — it is the single highest-leverage lock on the hand-rolled
// Solana wire format integration. If a vector here ever disagrees with our
// Rust output, the RUST CODE is what is wrong, not this fixture.
//
// compact-u16 (ShortU16 / short_vec length prefix) note: @solana/web3.js does
// not export its internal `encodeLength` helper (utils/shortvec-encoding.ts),
// and its public `Message.serialize()` allocates a fixed PACKET_DATA_SIZE
// (1232-byte) buffer, so we cannot drive a real full-message serialize() up to
// the 3-byte-varint boundary (16384) without exceeding that buffer. Instead we
// extract the EXACT `encodeLength` function body, verbatim, from the pinned
// dependency's own shipped bundle (node_modules/@solana/web3.js/lib/index.cjs.js)
// at generation time and execute it directly — this is still real-web3.js
// ground truth (the literal code the library runs internally to encode every
// compact-u16 length prefix in a serialized message), just reached through its
// unexported internal function rather than a public entry point.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { Keypair, PublicKey, SystemProgram, Transaction } from '@solana/web3.js';
import bs58 from 'bs58';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ─── Fixed, deterministic keypairs (reproducible across regenerations) ───────
const custody = Keypair.fromSeed(new Uint8Array(32).fill(0x11)); // vault custody / transfer source
const settlement = Keypair.fromSeed(new Uint8Array(32).fill(0x22)); // fee payer / nonce authority
const destination = new PublicKey(new Uint8Array(32).fill(0x33)); // claim settlement destination (not a signer)
const nonceAccount = new PublicKey(new Uint8Array(32).fill(0x44)); // durable-nonce account (not a signer here)
const plainBlockhash58 = new PublicKey(new Uint8Array(32).fill(0x55)).toBase58(); // stand-in recent blockhash
const durableNonce58 = new PublicKey(new Uint8Array(32).fill(0x66)).toBase58(); // stand-in durable nonce value

const LAMPORTS = 250_000_000; // 0.25 SOL, arbitrary distinct amount

// ─── Vector 1: base58 address encoding from a known 32-byte Ed25519 pubkey ───
const addressVector = {
  pubkey_hex: Buffer.from(custody.publicKey.toBytes()).toString('hex'),
  address_base58: custody.publicKey.toBase58(),
};

// ─── Vector 2: legacy message serialization for a plain System transfer ──────
// Mirrors chains::sol's plain (non-durable-nonce) transfer shape: a single
// signer, `from` is both the fee payer AND the transfer source (matches
// chains::solana::tx::build_transfer_message / system_transfer_instruction).
const plainTx = new Transaction({
  feePayer: custody.publicKey,
  recentBlockhash: plainBlockhash58,
}).add(
  SystemProgram.transfer({
    fromPubkey: custody.publicKey,
    toPubkey: destination,
    lamports: LAMPORTS,
  }),
);
const plainMessage = plainTx.compileMessage();
const legacyTransferVector = {
  from_pubkey_hex: Buffer.from(custody.publicKey.toBytes()).toString('hex'),
  to_pubkey_hex: Buffer.from(destination.toBytes()).toString('hex'),
  lamports: LAMPORTS,
  recent_blockhash_base58: plainBlockhash58,
  message_hex: Buffer.from(plainMessage.serialize()).toString('hex'),
};

// ─── Vector 3: durable-nonce transfer message bytes ───────────────────────────
// The exact two-signer shape chains::sol::adapter::sign_sol_payment_from builds:
// [ advance_nonce_account(nonce, authority=settlement), transfer(custody->dest) ],
// fee payer = settlement (distinct from the transfer source, custody).
const nonceTx = new Transaction({
  feePayer: settlement.publicKey,
  recentBlockhash: durableNonce58, // the durable nonce IS the "recent blockhash" field
}).add(
  SystemProgram.nonceAdvance({
    noncePubkey: nonceAccount,
    authorizedPubkey: settlement.publicKey,
  }),
  SystemProgram.transfer({
    fromPubkey: custody.publicKey,
    toPubkey: destination,
    lamports: LAMPORTS,
  }),
);
const nonceMessage = nonceTx.compileMessage();
const durableNonceTransferVector = {
  settlement_pubkey_hex: Buffer.from(settlement.publicKey.toBytes()).toString('hex'),
  custody_pubkey_hex: Buffer.from(custody.publicKey.toBytes()).toString('hex'),
  nonce_account_pubkey_hex: Buffer.from(nonceAccount.toBytes()).toString('hex'),
  destination_pubkey_hex: Buffer.from(destination.toBytes()).toString('hex'),
  lamports: LAMPORTS,
  durable_nonce_base58: durableNonce58,
  num_required_signatures: nonceMessage.header.numRequiredSignatures,
  // account_keys[0..num_required_signatures], in wire order — the order the
  // signatures below must appear in.
  required_signer_pubkeys_base58: nonceMessage.accountKeys
    .slice(0, nonceMessage.header.numRequiredSignatures)
    .map((k) => k.toBase58()),
  message_hex: Buffer.from(nonceMessage.serialize()).toString('hex'),
};

// ─── Vector 4: wire-transaction assembly with two signatures ─────────────────
// Sign the SAME durable-nonce transaction with both required keypairs and take
// the SDK's own full wire serialize() — ground truth for
// chains::solana::tx::assemble_wire_tx_multi + order_signatures_by_signer.
nonceTx.sign(settlement, custody);
const wireTxVector = {
  settlement_signature_hex: Buffer.from(
    nonceTx.signatures.find((s) => s.publicKey.equals(settlement.publicKey)).signature,
  ).toString('hex'),
  custody_signature_hex: Buffer.from(
    nonceTx.signatures.find((s) => s.publicKey.equals(custody.publicKey)).signature,
  ).toString('hex'),
  // The transaction id / first-signature-base58 the network and explorers use.
  first_signature_base58: bs58.encode(nonceTx.signatures[0].signature),
  wire_tx_hex: Buffer.from(nonceTx.serialize()).toString('hex'),
};

// ─── Vector 5: compact-u16 encoding edge cases ────────────────────────────────
// Extract the REAL `encodeLength` function body verbatim from the pinned
// dependency's own shipped bundle (see module doc comment) and execute it.
const web3Bundle = readFileSync(
  join(__dirname, 'node_modules', '@solana', 'web3.js', 'lib', 'index.cjs.js'),
  'utf8',
);
const match = web3Bundle.match(/function encodeLength\(bytes, len\) \{[\s\S]*?\n\}/);
if (!match) {
  throw new Error(
    'could not locate encodeLength in the pinned @solana/web3.js bundle; ' +
      'the package version may have changed its internal layout',
  );
}
// eslint-disable-next-line no-new-func
const encodeLength = new Function(
  'bytes',
  'len',
  match[0].replace(/^function encodeLength\(bytes, len\) \{/, '').replace(/\}$/, ''),
);
function encodeLengthHex(len) {
  const bytes = [];
  encodeLength(bytes, len);
  return Buffer.from(bytes).toString('hex');
}
const compactU16Vectors = [0, 1, 127, 128, 129, 200, 255, 256, 16383, 16384, 16385, 65535].map(
  (len) => ({ value: len, encoded_hex: encodeLengthHex(len) }),
);

// ─── Emit ──────────────────────────────────────────────────────────────────────
console.log(
  JSON.stringify(
    {
      comment:
        'Ground truth from @solana/web3.js 1.98.4. Regenerate: cd tools/sol-kat && npm install && node index.mjs',
      address: addressVector,
      legacy_transfer: legacyTransferVector,
      durable_nonce_transfer: durableNonceTransferVector,
      wire_tx: wireTxVector,
      compact_u16: compactU16Vectors,
    },
    null,
    2,
  ),
);
