// Generate known-answer test vectors for the backend's hand-rolled XRP Ledger
// signing path (src/rumi_protocol_backend/src/chains/xrp), using the official
// xrpl.js SDK (+ ripple-binary-codec) as ground truth.
//
// Regenerate (from the project root):
//   cd tools/xrp-kat && npm install && node index.mjs \
//     > ../../src/rumi_protocol_backend/src/chains/xrp/testdata/xrp_kat.json
//
// The Rust unit tests (codec/sign/address) assert byte-for-byte equality against
// this file — it is the single highest-leverage lock on the integration.
import { Wallet } from 'xrpl';
import { encode, encodeForSigning, decode } from 'ripple-binary-codec';

// Fixed Ed25519 wallet (deterministic). The 'sEd...' seed prefix is Ed25519.
const wallet = Wallet.fromSeed('sEdSKaCy2JT7JaM7v95H9SxkhP9wS2r', { algorithm: 'ed25519' });

// SigningPubKey is the 33-byte 0xED-prefixed key xrpl exposes as hex.
const pubkeyHex = wallet.publicKey; // "ED...."
const ed25519Raw = pubkeyHex.slice(2); // strip the ED flag -> 32-byte raw key

const tx = {
  TransactionType: 'Payment',
  Account: wallet.classicAddress,
  Destination: 'rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo',
  Amount: '1000000',            // 1 XRP in drops
  Fee: '20',                    // matches the rail's fixed Phase-1 fee (XRP_FEE_DROPS)
  Sequence: 42,
  LastLedgerSequence: 9000075,
  DestinationTag: 12345,
  SigningPubKey: pubkeyHex,
};

const signed = wallet.sign(tx); // { tx_blob, hash }

// We want BOTH:
//  - the raw serialized unsigned tx (no prefix)  -> for the codec test
//  - the full signing message (with the 0x53545800 prefix) -> for the sign test
const unsignedNoPrefix = encode({ ...tx }); // includes SigningPubKey, no TxnSignature
const signingMessageHex = encodeForSigning(tx); // 53545800 ‖ unsignedNoPrefix

// TxnSignature is the VL blob the signed tx carries; recover it from the signed blob.
const txnSignatureHex = decode(signed.tx_blob).TxnSignature;

console.log(JSON.stringify({
  comment: 'Ground truth from xrpl.js. Regenerate: cd tools/xrp-kat && npm install && node index.mjs',
  keypair: {
    ed25519_pubkey_hex: ed25519Raw,
    signing_pubkey_hex: pubkeyHex,
    classic_address: wallet.classicAddress,
  },
  payment: {
    account: tx.Account,
    destination: tx.Destination,
    amount_drops: Number(tx.Amount),
    fee_drops: Number(tx.Fee),
    sequence: tx.Sequence,
    last_ledger_sequence: tx.LastLedgerSequence,
    destination_tag: tx.DestinationTag,
  },
  unsigned_blob_hex: unsignedNoPrefix,
  signing_message_hex: signingMessageHex,
  txn_signature_hex: txnSignatureHex,
  signed_blob_hex: signed.tx_blob,
  tx_hash_hex: signed.hash,
}, null, 2));
