# ckERC20 token logo provenance

The SVGs in `src/vault_frontend/static/tokens/ckerc20/` were read from each
ledger's official `icrc1_metadata` query, using its `icrc1:logo` value, on
2026-09-29 against the `mainnet-live` environment. The retrieval script parsed
the SVG and rejected script, foreign object, image, and use elements, event
attributes, and non-fragment hrefs before saving the artwork. No third-party
logo files or inline data URLs are used by the selector.

| Symbol | Ledger canister |
| --- | --- |
| ckETH | `ss2fx-dyaaa-aaaar-qacoq-cai` |
| ckUSDC | `xevnm-gaaaa-aaaar-qafnq-cai` |
| ckUSDT | `cngnf-vqaaa-aaaar-qag4q-cai` |
| ckEURC | `pe5t5-diaaa-aaaar-qahwa-cai` |
| ckXAUT | `nza5v-qaaaa-aaaar-qahzq-cai` |
| ckLINK | `g4tto-rqaaa-aaaar-qageq-cai` |
| ckPEPE | `etik7-oiaaa-aaaar-qagia-cai` |
| ckOCT | `ebo5g-cyaaa-aaaar-qagla-cai` |
| ckSHIB | `fxffn-xiaaa-aaaar-qagoa-cai` |
| ckWBTC | `bptq2-faaaa-aaaar-qagxq-cai` |
| ckWSTETH | `j2tuh-yqaaa-aaaar-qahcq-cai` |
| ckUNI | `ilzky-ayaaa-aaaar-qahha-cai` |
| ckBAT | `j7x7x-syaaa-aaaar-qcbea-cai` |

`src/vault_frontend/src/lib/utils/ckerc20Logos.ts` maps only these known
symbols to local assets. Unknown token symbols intentionally have no logo and
remain identified by their ticker.
