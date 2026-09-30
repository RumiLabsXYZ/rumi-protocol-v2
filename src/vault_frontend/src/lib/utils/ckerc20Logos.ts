// Official artwork retrieved from each token ledger's icrc1:logo metadata;
// provenance and ledger IDs are recorded in docs/ckerc20-token-logos.md.
// Logo lookup is presentation only; supported assets still come from the live minter.
const symbols = ['ckETH', 'ckUSDC', 'ckUSDT', 'ckEURC', 'ckXAUT', 'ckLINK', 'ckPEPE', 'ckOCT', 'ckSHIB', 'ckWBTC', 'ckWSTETH', 'ckUNI', 'ckBAT'];
export function ckErc20Logo(symbol: string): string | undefined {
  return symbols.includes(symbol) ? `/tokens/ckerc20/${symbol}.svg` : undefined;
}
export const featuredCkErc20Symbols = ['ckUSDC', 'ckUSDT', 'ckEURC', 'ckXAUT'];
