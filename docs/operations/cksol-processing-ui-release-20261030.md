# ckSOL processing UI release — 2026-10-30

PR #599 is merged at `3dd4c2ee34c6a7ca7741c351d793ec52e52493fb`; its tree matches tested source `4d82c9001760d907e96f043e8bdd4409c7971381`. The 79 focused ckSOL tests, frontend build, and generated auth/custom-domain asset checks passed. One focused reviewer found the OISY SDK 200 ms channel lifetime issue; the explicit second processing click resolved it and passed targeted review.

Rob explicitly approved assets-only frontend publication. Target: existing `vault_frontend` (`tcfua-yaaaa-aaaap-qrd7q-cai`), operator `rumi_identity`. No backend installation, controller/permission change, sponsorship change, wallet approval, or financial dispatch was performed by the agent.

The dfx deploy wrapper failed locally because its Intel-emulated child Node expected x64 Rollup while the successful native frontend build used ARM dependencies. No direct Rollup dependency was added. The later assets-only dfx uploader's batch commit trapped with `asset already exists`; live index readback remained the old release. The owning cause was confirmed in the deployed Candid: `list` accepts optional `start` and `length`, defaults to 100 entries, while dfx 0.29.2 lists with an empty record. Full pagination returned 726 assets.

The compatible publication read the full inventory and prepared one atomic batch: 93 absent hashed files created, 99 identity contents updated, and the existing `/index.html` gzip encoding updated. No DeleteAsset, Clear, permission operation or Wasm installation was included. New file properties were copied from existing immutable assets of the same extension; existing properties stayed unchanged. Reviewer verification caught the necessary gzip index update before publication, then confirmed its new payload. The complete Candid argument encoded to roughly 1.053 MiB. Operations SHA256: `01bd90f59cce58b5357936d6d946a2b84463c43b0e5609d0ed20bb04e7c94cb0`.

Asset batch `622` committed successfully. Full post-readback verified all 99 changed identity hashes, the index gzip hash, all 726 prior keys and their content-type/header/access/alias properties, and total 819 keys. Public served index, app entry, start entry and ckSOL node bytes exactly matched the source-matched build.

- Index SHA256: `e9e61bc8f0ce06d968be1a19decf586744a38b97957413d18967bf898b8a8cb3`.
- Index gzip SHA256: `afd4b1be4036ed378f6751f661bd26b9b9fe0055cd7d03af2fd9d8e6fa5ee556`.
- ckSOL node SHA256: `c6e9b7e34356c19c503c23105af430163437460dc6aa770f829420402ee73a08`.
- Frontend remained Running with unchanged Wasm `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62` and controllers.

Authenticated browser readback loaded the new processing-cost language and prominent refresh control. The exact old pre-send popup failure reconciled to NotStarted with its original journal retained. Actual user-funded minting and refund settlement remain separate, pending user wallet consent and receipt readback. See DD-CKSOL-20261030-01.

Proposed durable engineering lesson, supported by installed SDK and deployed Candid: opening an OISY channel once does not span signed actions separated by network reads because the SDK defaults to auto-close after 200 ms. Also read the live asset interface and paginate its inventory before synchronizing; a list of exactly 100 assets may be a default page, not the complete inventory.
