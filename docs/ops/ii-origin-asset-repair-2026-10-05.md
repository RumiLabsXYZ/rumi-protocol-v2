# Internet Identity origin asset repair — 2026-10-05

Internet Identity sign-in from `https://app.rumiprotocol.com` failed with
"unverified origin" / "Unable to connect". The frontend page itself returned
HTTP 200, but the certified `/.well-known/ii-alternative-origins` endpoint on
both the custom domain and the derivation origin returned HTTP 503
`backend_response_verification`. The raw endpoint returned HTTP 404. A full
paginated live asset inventory contained 708 assets and neither well-known file.

## Cause and prevention

The repository kept `ii-alternative-origins` and `ic-domains` under
`src/vault_frontend/assets/.well-known/`, outside Svelte's configured `static/`
directory. The `icp deploy` recipe copied them after building; a plain
`npm run build --workspace vault_frontend` followed by `icp sync vault_frontend`
did not run that copy. Sync reconciles the remote asset set with `dist/`, so
this workflow could remove the files. The main checkout's current `dist/` also
lacked them. This is the supported likely explanation; the exact historical
deployment that removed them has not been established.

Move both files unchanged into `static/.well-known/`, remove the frontend
recipe's obsolete delete/copy steps, and run a postbuild verifier for their
presence, byte equality, custom-domain authorization, and JSON/CORS upload
configuration. Preserve the existing derivation origin and every allowlisted
origin; changing the derivation origin could change users' app principals.

## Narrow live repair

- Environment: `mainnet-live`; canister: `vault_frontend`,
  `tcfua-yaaaa-aaaap-qrd7q-cai`.
- Identity: `rumi_identity`, principal
  `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`;
  verified as an existing controller before the update.
- Read deployed `candid:service` metadata; it explicitly supports inline
  `last_chunk` in `SetAssetContentArguments` and paginated `list` requests.
- Created batch 611 and committed four operations atomically: `CreateAsset`
  and `SetAssetContent` for each missing file. The commit returned `()`.
  No full directory sync or Wasm install was performed.
- `ii-alternative-origins`: 385 bytes, SHA-256
  `01d37f58e287d02a74d7767b81d8d214de14d14f062654d660b2b5da3aadd8fd`.
- `ic-domains`: 58 bytes, SHA-256
  `d04261cb158bd5c519857f330ae347351cfbfe0ffbf2a2e1c3a7974065c08cf4`.

Both payloads are byte-identical to the files in `origin/main` at base commit
`40e23457570473041ca0332b7ccb86fa1f6975f0`.

## Verification

- Production frontend build and npm postbuild verification passed.
- Negative checks rejected an output missing either required file and an
  output with a changed II origin allowlist.
- `git diff --check`, Node syntax validation, and `icp project show` passed.
- Certified HTTPS requests to both the custom-domain and `icp0.io`
  `ii-alternative-origins` endpoints returned HTTP 200, `application/json`,
  `Access-Control-Allow-Origin: *`, and the exact expected body. The domain
  endpoint returned certified HTTP 200 with its exact body.
- Full paginated inventories before and after showed exactly two additions:
  all 708 existing records, including content hashes, timestamps and
  properties, were unchanged. The final inventory contains 710 assets.
- Live index HTML, all five controllers and the frontend Wasm hash remained
  unchanged. Wasm hash:
  `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`.
- A browser login attempt reached Internet Identity's normal
  "Continue to app.rumiprotocol.com" account-selection screen. No account
  consent, authenticated delegation, wallet transaction or funds movement was
  completed as part of this verification.

Two independent reviewers checked the source fix and narrow repair. The initial
question about deployed inline-chunk support was resolved by reading live
Candid metadata. Live endpoint and unchanged-inventory evidence were then
provided for the final review round.
