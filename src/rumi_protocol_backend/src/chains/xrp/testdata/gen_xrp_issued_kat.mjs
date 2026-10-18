// Golden-vector generator for the XRPL issued-token (icUSD) codec.
//
// The Rust tests in chains/xrp/issued_codec.rs lock our hand-rolled serializer
// byte-for-byte against the official `xrpl` npm package (ripple-binary-codec).
//
// Usage (the SDK is deliberately NOT a repo dependency; install it in a scratch dir):
//   mkdir -p /private/tmp/claude-501/xrpl-fixturegen && cd /private/tmp/claude-501/xrpl-fixturegen
//   npm init -y && npm install xrpl@5.3.0 --save-exact
//   XRPL_FIXTUREGEN_DIR=/private/tmp/claude-501/xrpl-fixturegen \
//     node <repo>/src/rumi_protocol_backend/src/chains/xrp/testdata/gen_xrp_issued_kat.mjs \
//     > <repo>/src/rumi_protocol_backend/src/chains/xrp/testdata/xrp_issued_kat.json
//
// Keys are derived from fixed, public, TEST-ONLY entropy; they never hold value.
// No network access happens here: encoding and ed25519 signing are pure functions.
import { createRequire } from 'node:module';

const dir = process.env.XRPL_FIXTUREGEN_DIR;
if (!dir) throw new Error('set XRPL_FIXTUREGEN_DIR to the scratch dir holding node_modules/xrpl');
const require = createRequire(dir + '/package.json');
const xrpl = require('xrpl');
const lock = require(dir + '/package-lock.json');

const ICUSD = '6963555344000000000000000000000000000000';
const ISSUER_ENTROPY = Uint8Array.from({ length: 16 }, (_, i) => 0x10 + i);
const HOLDER_ENTROPY = Uint8Array.from({ length: 16 }, (_, i) => 0xa0 + i);
const issuer = xrpl.Wallet.fromEntropy(ISSUER_ENTROPY, { algorithm: xrpl.ECDSA.ed25519 });
const holder = xrpl.Wallet.fromEntropy(HOLDER_ENTROPY, { algorithm: xrpl.ECDSA.ed25519 });

const FULLY_CANONICAL = 2147483648;
const iou = (value) => ({ currency: ICUSD, issuer: issuer.classicAddress, value });

function payment(name, from, to, value, extra = {}) {
  return {
    name,
    kind: 'payment',
    tx: {
      TransactionType: 'Payment',
      Account: from.classicAddress,
      Destination: to.classicAddress,
      Amount: iou(value),
      Fee: '12',
      Sequence: 7,
      LastLedgerSequence: 9000100,
      ...extra,
    },
    signer: from,
  };
}
function accountSet(name, extra) {
  return {
    name,
    kind: 'account_set',
    tx: {
      TransactionType: 'AccountSet',
      Account: issuer.classicAddress,
      Fee: '12',
      Sequence: 3,
      LastLedgerSequence: 9000100,
      Flags: FULLY_CANONICAL,
      ...extra,
    },
    signer: issuer,
  };
}

const cases = [
  payment('issuer_payment', issuer, holder, '100.5', { Flags: FULLY_CANONICAL }),
  payment('direct_issuer_return_with_tag', holder, issuer, '25.12345678', { DestinationTag: 4242 }),
  payment('max_normal_amount', issuer, holder, '9999999.99999999', { DestinationTag: 0, Flags: FULLY_CANONICAL }),
  payment('sub_e8s_amount', holder, issuer, '0.0000000012345678', { DestinationTag: 77 }),
  payment('scientific_small', holder, issuer, '1.5e-7', { DestinationTag: 1 }),
  payment('scientific_large', issuer, holder, '9999999999999999e80'),
  payment('min_exponent', issuer, holder, '1000000000000000e-96'),
  payment('whole_number_scientific', issuer, holder, '1e15'),
  payment('zero_amount', issuer, holder, '0'),
  accountSet('account_set_default_ripple', { SetFlag: 8 }),
  accountSet('account_set_require_dest', { SetFlag: 1 }),
  accountSet('account_set_transfer_rate_none', { TransferRate: 1000000000 }),
  accountSet('account_set_clear_flag', { ClearFlag: 8 }),
  {
    name: 'trust_set',
    kind: 'trust_set',
    tx: {
      TransactionType: 'TrustSet',
      Account: holder.classicAddress,
      LimitAmount: iou('1000000'),
      Fee: '12',
      Sequence: 9,
      LastLedgerSequence: 9000100,
      Flags: FULLY_CANONICAL,
    },
    signer: holder,
  },
];

const vectors = cases.map(({ name, kind, tx, signer }) => {
  const withKey = { ...tx, SigningPubKey: signer.publicKey };
  const signingHex = xrpl.encodeForSigning(withKey).toUpperCase();
  const signed = signer.sign({ ...tx });
  const decoded = xrpl.decode(signed.tx_blob);
  const amt = tx.Amount ?? tx.LimitAmount;
  return {
    name,
    kind,
    tx,
    signing_message_hex: signingHex,
    txn_signature_hex: decoded.TxnSignature.toUpperCase(),
    signed_blob_hex: signed.tx_blob.toUpperCase(),
    tx_hash: signed.hash.toUpperCase(),
    sdk_decoded_value: amt ? decoded.Amount?.value ?? decoded.LimitAmount?.value : null,
  };
});

const out = {
  comment:
    'Ground truth from the official xrpl npm package. Regenerate with gen_xrp_issued_kat.mjs (see its header). Test-only deterministic ed25519 keys.',
  sdk: {
    package: 'xrpl',
    version: lock.packages['node_modules/xrpl'].version,
    integrity: lock.packages['node_modules/xrpl'].integrity,
    binary_codec_version: lock.packages['node_modules/ripple-binary-codec'].version,
    binary_codec_integrity: lock.packages['node_modules/ripple-binary-codec'].integrity,
  },
  currency_hex: ICUSD,
  issuer: {
    entropy_hex: Buffer.from(ISSUER_ENTROPY).toString('hex'),
    classic_address: issuer.classicAddress,
    signing_pubkey_hex: issuer.publicKey.toUpperCase(),
  },
  holder: {
    entropy_hex: Buffer.from(HOLDER_ENTROPY).toString('hex'),
    classic_address: holder.classicAddress,
    signing_pubkey_hex: holder.publicKey.toUpperCase(),
  },
  vectors,
};
console.log(JSON.stringify(out, null, 2));
