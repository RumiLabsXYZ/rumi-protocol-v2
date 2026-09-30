import { Actor, AnonymousIdentity, HttpAgent } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { CANISTER_IDS, CONFIG } from '../config';
import { walletStore } from '../stores/wallet';
import { idlFactory as ledgerIdl } from '../idls/ledger.idl.js';
import { idlFactory as minterIdl } from '../idls/ckerc20_minter.idl.js';

export interface CkErc20TokenConfig {
  symbol: string;
  decimals: number;
  ledgerId: string;
  erc20Address: string;
  minimumDepositAmount: bigint | null;
}

export const CKERC20_MINTER_DASHBOARD = 'https://sv3dd-oaaaa-aaaar-qacoa-cai.raw.icp0.io/dashboard';

let queryAgent: HttpAgent | null = null;

async function getQueryAgent(): Promise<HttpAgent> {
  if (!queryAgent) {
    queryAgent = new HttpAgent({ host: CONFIG.host, identity: new AnonymousIdentity() });
    if (CONFIG.isLocal) await queryAgent.fetchRootKey();
  }
  return queryAgent;
}

export async function getCkErc20MinterActor(): Promise<any> {
  const agent = await getQueryAgent();
  return Actor.createActor(minterIdl as any, { agent, canisterId: CANISTER_IDS.CKERC20_MINTER });
}

export async function getCkErc20LedgerActor(ledgerId: string): Promise<any> {
  const agent = await getQueryAgent();
  return Actor.createActor(ledgerIdl as any, { agent, canisterId: ledgerId });
}

export async function getWalletLedgerActor(ledgerId: string): Promise<any> {
  return walletStore.getActor(ledgerId, ledgerIdl);
}

export async function getWalletCkErc20MinterActor(): Promise<any> {
  return walletStore.getActor(CANISTER_IDS.CKERC20_MINTER, minterIdl);
}

export async function discoverCkErc20Tokens(info: any): Promise<CkErc20TokenConfig[]> {
  const supported = info.supported_ckerc20_tokens?.[0] ?? [];
  if (!supported.length) throw new Error('The live ckETH minter did not return any supported ckERC20 tokens.');
  const minimums = new Map<string, bigint>(
    (info.minimum_deposit_amounts?.[0] ?? []).map((item: any) => [
      String(item.erc20_contract_address).toLowerCase(), BigInt(item.minimum_deposit_amount),
    ]),
  );
  return Promise.all(supported.map(async (candidate: any) => {
    const symbol = String(candidate.ckerc20_token_symbol ?? '').trim();
    const ledgerId = candidate.ledger_canister_id?.toText?.() ?? '';
    const erc20Address = String(candidate.erc20_contract_address ?? '').trim();
    if (!/^ck[A-Za-z0-9]+$/.test(symbol) || !ledgerId || !validateEthereumAddress(erc20Address)) {
      throw new Error(`The minter returned invalid metadata for supported token ${symbol || '(unknown)'}.`);
    }
    const ledger = await getCkErc20LedgerActor(ledgerId);
    const [ledgerSymbol, decimalsValue] = await Promise.all([ledger.icrc1_symbol(), ledger.icrc1_decimals()]);
    const decimals = Number(decimalsValue);
    if (String(ledgerSymbol).trim() !== symbol) {
      throw new Error(`${symbol} metadata does not match its ledger symbol (${String(ledgerSymbol)}).`);
    }
    if (!Number.isInteger(decimals) || decimals < 0 || decimals > 18) {
      throw new Error(`${symbol} ledger returned an unsupported decimal count (${String(decimalsValue)}).`);
    }
    return {
      symbol,
      decimals,
      ledgerId,
      erc20Address,
      minimumDepositAmount: minimums.get(erc20Address.toLowerCase()) ?? null,
    };
  }));
}

export function assertTokenSupported(info: any, config: CkErc20TokenConfig) {
  const token = (info.supported_ckerc20_tokens?.[0] ?? []).find(
    (candidate: any) => candidate.ledger_canister_id.toText() === config.ledgerId,
  );
  if (!token) throw new Error(`The live ckETH minter does not currently list ${config.symbol} as supported.`);
  if (token.erc20_contract_address.toLowerCase() !== config.erc20Address.toLowerCase()) {
    throw new Error(`The minter reports a different Ethereum token contract for ${config.symbol}. Deposit is disabled.`);
  }
  return token;
}

export function parseTokenAmount(value: string, decimals: number): bigint {
  const input = value.trim();
  if (!/^(?:0|[1-9]\d*)(?:\.\d+)?$/.test(input)) throw new Error('Enter a valid positive amount.');
  const [whole, fraction = ''] = input.split('.');
  if (fraction.length > decimals) throw new Error(`Use no more than ${decimals} decimal places.`);
  const amount = BigInt(whole) * 10n ** BigInt(decimals) + BigInt((fraction + '0'.repeat(decimals)).slice(0, decimals) || '0');
  if (amount <= 0n) throw new Error('Amount must be greater than zero.');
  return amount;
}

export function formatTokenAmount(amount: bigint, decimals = 6, maxFraction = 6): string {
  const scale = 10n ** BigInt(decimals);
  const whole = amount / scale;
  const fraction = (amount % scale).toString().padStart(decimals, '0').slice(0, maxFraction).replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

export function validateEthereumAddress(address: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(address.trim());
}

export function principalToBytes32(principal: Principal): string {
  const bytes = principal.toUint8Array();
  if (bytes.length > 31) throw new Error('Connected principal does not fit the minter helper bytes32 format.');
  const encoded = new Uint8Array(32);
  // DFINITY's live minter dashboard encodes Principal as a length byte, then
  // the principal bytes, then zero padding (verified against its converter).
  encoded[0] = bytes.length;
  encoded.set(bytes, 1);
  return `0x${Array.from(encoded, (byte) => byte.toString(16).padStart(2, '0')).join('')}`;
}

export const DEFAULT_EVM_SUBACCOUNT = `0x${'00'.repeat(32)}`;

export function encodeUint256(value: bigint): string {
  if (value < 0n || value >= 1n << 256n) throw new Error('Amount is outside the uint256 range.');
  return value.toString(16).padStart(64, '0');
}

export function encodeAddressWord(address: string): string {
  if (!validateEthereumAddress(address)) throw new Error('Invalid Ethereum address.');
  return address.slice(2).toLowerCase().padStart(64, '0');
}

export function encodeDepositErc20(config: CkErc20TokenConfig, amount: bigint, principal: Principal): string {
  // depositErc20(address,uint256,bytes32,bytes32), selector verified against
  // the deployed DFINITY helper ABI; Principal uses the minter's bytes32 encoding.
  return `0xdb9751af${encodeAddressWord(config.erc20Address)}${encodeUint256(amount)}${principalToBytes32(principal).slice(2)}${DEFAULT_EVM_SUBACCOUNT.slice(2)}`;
}

export interface CkErc20WithdrawalQuote {
  owner: Principal;
  tokenLedgerId: string;
  amount: bigint;
  ckTokenFee: bigint;
  ckEthFee: bigint;
  maxTransactionFee: bigint;
  ckTokenAllowance: bigint;
  ckEthAllowance: bigint;
  ckTokenBalance: bigint;
  ckEthBalance: bigint;
  quotedAtMs: number;
  minterPriceTimestampMs: number | null;
}

export async function getCkErc20WithdrawalQuote(
  token: CkErc20TokenConfig,
  amount: bigint,
  owner: Principal,
): Promise<CkErc20WithdrawalQuote> {
  const minter = await getCkErc20MinterActor();
  const ckTokenLedger = await getCkErc20LedgerActor(token.ledgerId);
  const ckEthLedger = await getCkErc20LedgerActor(CANISTER_IDS.CKETH_LEDGER);
  const tokenPrincipal = Principal.fromText(token.ledgerId);
  const [ckTokenFee, ckEthFee, feeQuote, ckTokenBalance, ckEthBalance] = await Promise.all([
    ckTokenLedger.icrc1_fee(),
    ckEthLedger.icrc1_fee(),
    minter.eip_1559_transaction_price([{ ckerc20_ledger_id: tokenPrincipal }]),
    ckTokenLedger.icrc1_balance_of({ owner, subaccount: [] }),
    ckEthLedger.icrc1_balance_of({ owner, subaccount: [] }),
  ]);
  const tokenFee = BigInt(ckTokenFee);
  const ethFee = BigInt(ckEthFee);
  const maxTransactionFee = BigInt(feeQuote.max_transaction_fee);
  return {
    owner,
    tokenLedgerId: token.ledgerId,
    amount,
    ckTokenFee: tokenFee,
    ckEthFee: ethFee,
    maxTransactionFee,
    ckTokenAllowance: amount + tokenFee,
    ckEthAllowance: maxTransactionFee + ethFee,
    ckTokenBalance: BigInt(ckTokenBalance),
    ckEthBalance: BigInt(ckEthBalance),
    quotedAtMs: Date.now(),
    minterPriceTimestampMs: feeQuote.timestamp?.[0] === undefined ? null : Number(BigInt(feeQuote.timestamp[0]) / 1_000_000n),
  };
}

function unwrapOk(result: any, label: string): bigint {
  if (result && 'Ok' in result) return BigInt(result.Ok);
  const tag = result && typeof result === 'object' ? Object.keys(result)[0] : 'Unknown';
  const details = result?.[tag];
  const text = typeof details === 'string' ? details : tag;
  throw new Error(`${label}: ${text}`);
}

export async function approveAndWithdrawCkErc20(params: {
  token: CkErc20TokenConfig;
  amount: bigint;
  recipient: string;
  owner: Principal;
  quote: CkErc20WithdrawalQuote;
  isLive: () => boolean;
  onWithdrawalSubmitted?: () => void;
  onWithdrawalResolved?: () => void;
}): Promise<{ ckEthApproveBlock: bigint; ckTokenApproveBlock: bigint; ckEthBurnBlock: bigint; ckTokenBurnBlock: bigint }> {
  const { token, amount, recipient, owner, quote, isLive, onWithdrawalSubmitted, onWithdrawalResolved } = params;
  if (!validateEthereumAddress(recipient)) throw new Error('Enter a valid Ethereum address.');
  if (quote.owner.toText() !== owner.toText() || quote.tokenLedgerId !== token.ledgerId || quote.amount !== amount) {
    throw new Error('The withdrawal quote does not match the current wallet, token, or amount. Refresh the quote.');
  }
  if (Date.now() - quote.quotedAtMs > 60_000) throw new Error('The withdrawal fee quote expired. Refresh it before approving.');
  const ckTokenPrincipal = Principal.fromText(token.ledgerId);
  const minterPrincipal = Principal.fromText(CANISTER_IDS.CKERC20_MINTER);
  if (!isLive()) throw new Error('Wallet changed before approval. Reconnect and review the request again.');
  const freshQuote = await getCkErc20WithdrawalQuote(token, amount, owner);
  if (!isLive()) throw new Error('Wallet changed while refreshing the withdrawal fees.');
  if (freshQuote.ckTokenFee !== quote.ckTokenFee || freshQuote.ckEthFee !== quote.ckEthFee || freshQuote.maxTransactionFee !== quote.maxTransactionFee) {
    throw new Error('The ledger fee or ckETH fee cap changed since the displayed quote. Refresh the quote before approving.');
  }
  if (freshQuote.ckEthBalance < freshQuote.ckEthAllowance + freshQuote.ckEthFee) {
    throw new Error(`Not enough ckETH for the displayed fee cap and approval fees. Required allowance: ${formatTokenAmount(freshQuote.ckEthAllowance, 18, 8)} ckETH.`);
  }
  if (freshQuote.ckTokenBalance < amount + freshQuote.ckTokenFee * 2n) {
    throw new Error(`Not enough ${token.symbol} for the amount and ICRC-2 approval/withdrawal fees. Current balance: ${formatTokenAmount(freshQuote.ckTokenBalance, token.decimals)} ${token.symbol}.`);
  }

  const expiresAt = BigInt(Date.now() + 10 * 60 * 1000) * 1_000_000n;
  const ckEthActor = await getWalletLedgerActor(CANISTER_IDS.CKETH_LEDGER);
  const ckEthApprove = await ckEthActor.icrc2_approve({
    from_subaccount: [], spender: { owner: minterPrincipal, subaccount: [] }, amount: freshQuote.ckEthAllowance,
    expected_allowance: [], expires_at: [expiresAt], fee: [freshQuote.ckEthFee], memo: [], created_at_time: [],
  });
  const ckEthApproveBlock = unwrapOk(ckEthApprove, 'ckETH approval failed');
  if (!isLive()) throw new Error(`ckETH approval confirmed at block ${ckEthApproveBlock}, but the connected wallet changed. No withdrawal was requested.`);

  const ckTokenActor = await getWalletLedgerActor(token.ledgerId);
  const ckTokenApprove = await ckTokenActor.icrc2_approve({
    from_subaccount: [], spender: { owner: minterPrincipal, subaccount: [] }, amount: freshQuote.ckTokenAllowance,
    expected_allowance: [], expires_at: [expiresAt], fee: [freshQuote.ckTokenFee], memo: [], created_at_time: [],
  });
  const ckTokenApproveBlock = unwrapOk(ckTokenApprove, `${token.symbol} approval failed`);
  if (!isLive()) throw new Error(`Both approvals were confirmed (ckETH ${ckEthApproveBlock}, ${token.symbol} ${ckTokenApproveBlock}), but the connected wallet changed. No withdrawal was requested.`);

  const minterActor = await getWalletCkErc20MinterActor();
  if (!isLive()) throw new Error('Wallet changed before the withdrawal request was submitted. No withdrawal was requested.');
  onWithdrawalSubmitted?.();
  const result = await minterActor.withdraw_erc20({
    amount,
    ckerc20_ledger_id: ckTokenPrincipal,
    recipient: recipient.trim(),
    from_cketh_subaccount: [],
    from_ckerc20_subaccount: [],
  });
  // A Candid response, including Err, resolves the request. A rejected call
  // leaves the caller's persisted reconciliation lock in place because the
  // minter may have processed it before the response was lost.
  onWithdrawalResolved?.();
  if ('Err' in result) {
    const errorTag = Object.keys(result.Err)[0];
    if (errorTag === 'CkErc20LedgerError') {
      const partial = result.Err.CkErc20LedgerError;
      throw new Error(`${token.symbol} withdrawal was rejected after ckETH processing. ckETH burn block: ${partial.cketh_block_index}. Ledger error: ${Object.keys(partial.error)[0]}.`);
    }
    throw new Error(`Withdrawal request rejected: ${errorTag}`);
  }
  return {
    ckEthApproveBlock,
    ckTokenApproveBlock,
    ckEthBurnBlock: BigInt(result.Ok.cketh_block_index),
    ckTokenBurnBlock: BigInt(result.Ok.ckerc20_block_index),
  };
}
