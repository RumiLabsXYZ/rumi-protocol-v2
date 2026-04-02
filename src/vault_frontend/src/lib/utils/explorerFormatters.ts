import {
  formatE8s, formatTokenAmount, formatPercent, formatCR, formatBps,
  timeAgo, formatTimestamp, getTokenSymbol, getTokenDecimals,
  shortenPrincipal, getCanisterName
} from '$utils/explorerHelpers';

// ─── Types ────────────────────────────────────────────────────────────

export type EventCategory =
  | 'vault_ops'
  | 'liquidation'
  | 'redemption'
  | 'stability_pool'
  | 'threepool'
  | 'amm'
  | 'admin'
  | 'system';

export type FieldType =
  | 'text'
  | 'amount'
  | 'usd'
  | 'percentage'
  | 'address'
  | 'vault'
  | 'token'
  | 'event'
  | 'timestamp'
  | 'json'
  | 'canister'
  | 'block_index'
  | 'ratio';

export interface EventField {
  label: string;
  value: string;
  type: FieldType;
  linkTarget?: string;
  tokenPrincipal?: string;
}

export interface FormattedEvent {
  summary: string;
  typeName: string;
  category: EventCategory;
  badgeColor: string;
  fields: EventField[];
}

// ─── Badge Colors ─────────────────────────────────────────────────────

export const BADGE_COLORS: Record<EventCategory, string> = {
  vault_ops: 'bg-blue-500/15 text-blue-400 border border-blue-500/30',
  liquidation: 'bg-red-500/15 text-red-400 border border-red-500/30',
  redemption: 'bg-purple-500/15 text-purple-400 border border-purple-500/30',
  stability_pool: 'bg-teal-500/15 text-teal-400 border border-teal-500/30',
  threepool: 'bg-cyan-500/15 text-cyan-400 border border-cyan-500/30',
  amm: 'bg-indigo-500/15 text-indigo-400 border border-indigo-500/30',
  admin: 'bg-amber-500/15 text-amber-400 border border-amber-500/30',
  system: 'bg-gray-500/15 text-gray-400 border border-gray-500/30',
};

// ─── Event Categories ─────────────────────────────────────────────────

export const EVENT_CATEGORIES: { key: EventCategory; label: string }[] = [
  { key: 'vault_ops', label: 'Vault Operations' },
  { key: 'liquidation', label: 'Liquidations' },
  { key: 'redemption', label: 'Redemptions' },
  { key: 'stability_pool', label: 'Stability Pool' },
  { key: 'threepool', label: '3Pool' },
  { key: 'admin', label: 'Admin' },
  { key: 'system', label: 'System' },
];

// ─── Category Lookup Tables ───────────────────────────────────────────

const VAULT_OPS_KEYS = new Set([
  'open_vault', 'close_vault', 'withdraw_and_close_vault',
  'vault_withdrawn_and_closed', 'VaultWithdrawnAndClosed',
  'borrow_from_vault', 'repay_to_vault', 'dust_forgiven',
  'add_margin_to_vault', 'collateral_withdrawn',
  'partial_collateral_withdrawn', 'margin_transfer',
]);

const LIQUIDATION_KEYS = new Set([
  'liquidate_vault', 'partial_liquidate_vault', 'redistribute_vault',
  'bot_liquidation_claimed', 'bot_liquidation_confirmed', 'bot_liquidation_canceled',
]);

const REDEMPTION_KEYS = new Set([
  'redemption_on_vaults', 'redemption_transfered', 'reserve_redemption',
]);

const STABILITY_POOL_KEYS = new Set([
  'provide_liquidity', 'withdraw_liquidity', 'claim_liquidity_returns',
]);

const SYSTEM_KEYS = new Set([
  'init', 'upgrade', 'accrue_interest',
]);

// ─── 3Pool Token Lookup ───────────────────────────────────────────────

const THREE_POOL_TOKEN_NAMES: Record<number, string> = { 0: 'icUSD', 1: 'ckUSDT', 2: 'ckUSDC' };
const THREE_POOL_TOKEN_DECIMALS: Record<number, number> = { 0: 8, 1: 6, 2: 6 };

function fmtPoolAmount(amount: bigint | number, tokenIndex: number): string {
  const decimals = THREE_POOL_TOKEN_DECIMALS[tokenIndex] ?? 8;
  const val = Number(amount) / Math.pow(10, decimals);
  return val.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 });
}

/**
 * Format a 3Pool SwapEvent into the same FormattedEvent shape used by protocol events.
 * SwapEvent: { id, fee, token_in, amount_out, timestamp, caller, amount_in, token_out }
 */
export function formatSwapEvent(swap: any): FormattedEvent {
  const tokenIn = THREE_POOL_TOKEN_NAMES[swap.token_in] ?? `token${swap.token_in}`;
  const tokenOut = THREE_POOL_TOKEN_NAMES[swap.token_out] ?? `token${swap.token_out}`;
  const amtIn = fmtPoolAmount(swap.amount_in, swap.token_in);
  const amtOut = fmtPoolAmount(swap.amount_out, swap.token_out);
  const feeAmt = fmtPoolAmount(swap.fee, swap.token_out);

  const fields: EventField[] = [
    { label: 'Token In', value: `${amtIn} ${tokenIn}`, type: 'amount' },
    { label: 'Token Out', value: `${amtOut} ${tokenOut}`, type: 'amount' },
    { label: 'Fee', value: `${feeAmt} ${tokenOut}`, type: 'amount' },
  ];

  const callerText = swap.caller?.toText?.() ?? swap.caller?.toString?.() ?? null;
  if (callerText) {
    fields.push({
      label: 'Caller',
      value: shortenPrincipal(callerText),
      type: 'address',
      linkTarget: callerText,
    });
  }

  if (swap.timestamp) {
    fields.push({ label: 'Timestamp', value: formatTimestamp(swap.timestamp), type: 'timestamp' });
  }

  return {
    summary: `Swapped ${amtIn} ${tokenIn} → ${amtOut} ${tokenOut}`,
    typeName: '3Pool Swap',
    category: 'threepool',
    badgeColor: BADGE_COLORS.threepool,
    fields,
  };
}

/**
 * Format an AMM swap event into FormattedEvent.
 */
export function formatAmmSwapEvent(event: any): FormattedEvent {
  const poolId = event.pool_id ?? '?';
  const tokenInPrincipal = event.token_in?.toText?.() ?? String(event.token_in ?? '');
  const tokenOutPrincipal = event.token_out?.toText?.() ?? String(event.token_out ?? '');
  const tokenInSym = getTokenSymbol(tokenInPrincipal);
  const tokenOutSym = getTokenSymbol(tokenOutPrincipal);
  const tokenInDec = getTokenDecimals(tokenInPrincipal);
  const tokenOutDec = getTokenDecimals(tokenOutPrincipal);
  const amountIn = event.amount_in != null ? formatTokenAmount(BigInt(event.amount_in), tokenInDec) : '?';
  const amountOut = event.amount_out != null ? formatTokenAmount(BigInt(event.amount_out), tokenOutDec) : '?';

  const fields: EventField[] = [
    { label: 'Token In', value: `${amountIn} ${tokenInSym}`, type: 'amount' },
    { label: 'Token Out', value: `${amountOut} ${tokenOutSym}`, type: 'amount' },
    { label: 'Pool', value: String(poolId), type: 'text' },
  ];

  if (tokenInPrincipal) fields.push({ label: 'Token In Ledger', value: shortenPrincipal(tokenInPrincipal), type: 'token', linkTarget: tokenInPrincipal });
  if (tokenOutPrincipal) fields.push({ label: 'Token Out Ledger', value: shortenPrincipal(tokenOutPrincipal), type: 'token', linkTarget: tokenOutPrincipal });

  const callerText = event.caller?.toText?.() ?? (typeof event.caller === 'string' ? event.caller : null);
  if (callerText) fields.push({ label: 'Caller', value: shortenPrincipal(callerText), type: 'address', linkTarget: callerText });

  if (event.timestamp) fields.push({ label: 'Timestamp', value: formatTimestamp(event.timestamp), type: 'timestamp', linkTarget: String(event.timestamp) });

  return {
    summary: `Swapped ${amountIn} ${tokenInSym} → ${amountOut} ${tokenOutSym}`,
    typeName: 'AMM Swap',
    category: 'amm',
    badgeColor: BADGE_COLORS.amm,
    fields,
  };
}

/**
 * Format an AMM liquidity event into FormattedEvent.
 */
export function formatAmmLiquidityEvent(event: any): FormattedEvent {
  const action = event.action ? Object.keys(event.action)[0] : '?';
  const poolId = event.pool_id ?? '?';
  const tokenAPrincipal = event.token_a?.toText?.() ?? String(event.token_a ?? '');
  const tokenBPrincipal = event.token_b?.toText?.() ?? String(event.token_b ?? '');
  const tokenA = getTokenSymbol(tokenAPrincipal);
  const tokenB = getTokenSymbol(tokenBPrincipal);
  const tokenADec = getTokenDecimals(tokenAPrincipal);
  const tokenBDec = getTokenDecimals(tokenBPrincipal);
  const amtA = event.amount_a != null ? formatTokenAmount(BigInt(event.amount_a), tokenADec) : '?';
  const amtB = event.amount_b != null ? formatTokenAmount(BigInt(event.amount_b), tokenBDec) : '?';
  const lpShares = event.lp_shares != null ? formatTokenAmount(BigInt(event.lp_shares), 8) : '?';

  const isAdd = action === 'AddLiquidity';
  const typeName = isAdd ? 'AMM Add Liquidity' : 'AMM Remove Liquidity';
  const summary = isAdd
    ? `Added ${amtA} ${tokenA} + ${amtB} ${tokenB} → ${lpShares} LP`
    : `Removed ${lpShares} LP → ${amtA} ${tokenA} + ${amtB} ${tokenB}`;

  const fields: EventField[] = [
    { label: 'Action', value: isAdd ? 'Add Liquidity' : 'Remove Liquidity', type: 'text' },
    { label: 'Token A', value: `${amtA} ${tokenA}`, type: 'amount' },
    { label: 'Token B', value: `${amtB} ${tokenB}`, type: 'amount' },
    { label: 'LP Shares', value: lpShares, type: 'amount' },
    { label: 'Pool', value: String(poolId), type: 'text' },
  ];

  if (tokenAPrincipal) fields.push({ label: 'Token A Ledger', value: shortenPrincipal(tokenAPrincipal), type: 'token', linkTarget: tokenAPrincipal });
  if (tokenBPrincipal) fields.push({ label: 'Token B Ledger', value: shortenPrincipal(tokenBPrincipal), type: 'token', linkTarget: tokenBPrincipal });

  const callerText = event.caller?.toText?.() ?? (typeof event.caller === 'string' ? event.caller : null);
  if (callerText) fields.push({ label: 'Caller', value: shortenPrincipal(callerText), type: 'address', linkTarget: callerText });

  if (event.timestamp) fields.push({ label: 'Timestamp', value: formatTimestamp(event.timestamp), type: 'timestamp', linkTarget: String(event.timestamp) });

  return {
    summary,
    typeName,
    category: 'amm',
    badgeColor: BADGE_COLORS.amm,
    fields,
  };
}

/**
 * Format an AMM admin event into FormattedEvent.
 */
export function formatAmmAdminEvent(event: any): FormattedEvent {
  const action = event.action ? Object.keys(event.action)[0] : 'Unknown';
  const data = event.action?.[action] ?? {};

  let summary = action;
  switch (action) {
    case 'CreatePool': summary = `Created pool ${data.pool_id}`; break;
    case 'SetFee': summary = `Set fee to ${data.fee_bps}bps on ${data.pool_id}`; break;
    case 'SetProtocolFee': summary = `Set protocol fee to ${data.protocol_fee_bps}bps on ${data.pool_id}`; break;
    case 'WithdrawProtocolFees': summary = `Withdrew protocol fees from ${data.pool_id}`; break;
    case 'PausePool': summary = `Paused pool ${data.pool_id}`; break;
    case 'UnpausePool': summary = `Unpaused pool ${data.pool_id}`; break;
    case 'SetPoolCreationOpen': summary = `Pool creation ${data.open ? 'opened' : 'closed'}`; break;
    case 'SetMaintenanceMode': summary = `Maintenance mode ${data.enabled ? 'enabled' : 'disabled'}`; break;
    case 'ClaimPending': summary = `Claimed pending #${data.claim_id}`; break;
    case 'ResolvePendingClaim': summary = `Resolved pending claim #${data.claim_id}`; break;
  }

  const fields: EventField[] = [
    { label: 'Action', value: action, type: 'text' },
  ];
  if (data.pool_id != null) fields.push({ label: 'Pool', value: String(data.pool_id), type: 'text' });
  if (data.fee_bps != null) fields.push({ label: 'Fee', value: `${data.fee_bps} bps`, type: 'text' });
  if (data.protocol_fee_bps != null) fields.push({ label: 'Protocol Fee', value: `${data.protocol_fee_bps} bps`, type: 'text' });

  const callerText = event.caller?.toText?.() ?? (typeof event.caller === 'string' ? event.caller : null);
  if (callerText) fields.push({ label: 'Caller', value: shortenPrincipal(callerText), type: 'address', linkTarget: callerText });

  if (event.timestamp) fields.push({ label: 'Timestamp', value: formatTimestamp(event.timestamp), type: 'timestamp', linkTarget: String(event.timestamp) });

  return {
    summary,
    typeName: 'AMM Admin',
    category: 'admin',
    badgeColor: BADGE_COLORS.admin,
    fields,
  };
}

/**
 * Format a 3Pool liquidity event into FormattedEvent.
 */
export function format3PoolLiquidityEvent(event: any): FormattedEvent {
  const action = event.action ? Object.keys(event.action)[0] : '?';
  const amounts = event.amounts ?? [];
  const lpAmount = event.lp_amount != null ? formatTokenAmount(BigInt(event.lp_amount), 8) : '?';
  const coinIndex = event.coin_index?.[0] ?? null;

  let summary = '';
  let typeName = '';
  const fields: EventField[] = [];

  switch (action) {
    case 'AddLiquidity': {
      const parts = amounts.map((a: any, i: number) => {
        const sym = THREE_POOL_TOKEN_NAMES[i] ?? `token${i}`;
        const amt = fmtPoolAmount(a, i);
        return Number(a) > 0 ? `${amt} ${sym}` : null;
      }).filter(Boolean).join(' + ');
      typeName = '3Pool Add Liquidity';
      summary = `Added ${parts || '0'} → ${lpAmount} LP`;
      fields.push({ label: 'Action', value: 'Add Liquidity', type: 'text' });
      for (let i = 0; i < amounts.length; i++) {
        if (Number(amounts[i]) > 0) {
          fields.push({ label: THREE_POOL_TOKEN_NAMES[i] ?? `token${i}`, value: fmtPoolAmount(amounts[i], i), type: 'amount' });
        }
      }
      fields.push({ label: 'LP Tokens Minted', value: lpAmount, type: 'amount' });
      break;
    }
    case 'RemoveLiquidity': {
      const parts = amounts.map((a: any, i: number) => {
        const sym = THREE_POOL_TOKEN_NAMES[i] ?? `token${i}`;
        const amt = fmtPoolAmount(a, i);
        return Number(a) > 0 ? `${amt} ${sym}` : null;
      }).filter(Boolean).join(' + ');
      typeName = '3Pool Remove Liquidity';
      summary = `Removed ${lpAmount} LP → ${parts || '0'}`;
      fields.push({ label: 'Action', value: 'Remove Liquidity', type: 'text' });
      fields.push({ label: 'LP Tokens Burned', value: lpAmount, type: 'amount' });
      for (let i = 0; i < amounts.length; i++) {
        if (Number(amounts[i]) > 0) {
          fields.push({ label: THREE_POOL_TOKEN_NAMES[i] ?? `token${i}`, value: fmtPoolAmount(amounts[i], i), type: 'amount' });
        }
      }
      break;
    }
    case 'RemoveOneCoin': {
      const idx = coinIndex ?? 0;
      const sym = THREE_POOL_TOKEN_NAMES[idx] ?? `token${idx}`;
      const amt = amounts[idx] != null ? fmtPoolAmount(amounts[idx], idx) : '?';
      typeName = '3Pool Remove One Coin';
      summary = `Removed ${lpAmount} LP → ${amt} ${sym}`;
      fields.push({ label: 'Action', value: 'Remove One Coin', type: 'text' });
      fields.push({ label: 'LP Tokens Burned', value: lpAmount, type: 'amount' });
      fields.push({ label: sym, value: amt, type: 'amount' });
      break;
    }
    case 'Donate': {
      const idx = coinIndex ?? 0;
      const sym = THREE_POOL_TOKEN_NAMES[idx] ?? `token${idx}`;
      const amt = amounts[idx] != null ? fmtPoolAmount(amounts[idx], idx) : '?';
      typeName = '3Pool Donate';
      summary = `Donated ${amt} ${sym}`;
      fields.push({ label: 'Action', value: 'Donate', type: 'text' });
      fields.push({ label: sym, value: amt, type: 'amount' });
      break;
    }
    default:
      typeName = `3Pool ${action}`;
      summary = action;
      fields.push({ label: 'Action', value: action, type: 'text' });
  }

  const callerText = event.caller?.toText?.() ?? (typeof event.caller === 'string' ? event.caller : null);
  if (callerText) fields.push({ label: 'Caller', value: shortenPrincipal(callerText), type: 'address', linkTarget: callerText });

  if (event.timestamp) fields.push({ label: 'Timestamp', value: formatTimestamp(event.timestamp), type: 'timestamp', linkTarget: String(event.timestamp) });

  return {
    summary,
    typeName,
    category: 'threepool',
    badgeColor: BADGE_COLORS.threepool,
    fields,
  };
}

/**
 * Format a 3Pool admin event into FormattedEvent.
 */
export function format3PoolAdminEvent(event: any): FormattedEvent {
  const action = event.action ? Object.keys(event.action)[0] : 'Unknown';
  const data = event.action?.[action] ?? {};

  let summary = action;
  switch (action) {
    case 'RampA': summary = `Ramping A to ${data.future_a}`; break;
    case 'StopRampA': summary = `Stopped A ramp at ${data.frozen_a}`; break;
    case 'WithdrawAdminFees': summary = 'Withdrew admin fees'; break;
    case 'SetPaused': summary = `Pool ${data.paused ? 'paused' : 'unpaused'}`; break;
    case 'SetSwapFee': summary = `Set swap fee to ${data.fee_bps}bps`; break;
    case 'SetAdminFee': summary = `Set admin fee to ${data.fee_bps}bps`; break;
    case 'AddAuthorizedBurnCaller': summary = `Added burn caller ${shortenPrincipal(data.canister?.toText?.() ?? '')}`; break;
    case 'RemoveAuthorizedBurnCaller': summary = `Removed burn caller ${shortenPrincipal(data.canister?.toText?.() ?? '')}`; break;
  }

  const fields: EventField[] = [
    { label: 'Action', value: action, type: 'text' },
  ];
  if (data.fee_bps != null) fields.push({ label: 'Fee', value: `${data.fee_bps} bps`, type: 'text' });
  if (data.future_a != null) fields.push({ label: 'Future A', value: String(data.future_a), type: 'text' });
  if (data.canister) {
    const p = data.canister.toText?.() ?? String(data.canister);
    fields.push({ label: 'Canister', value: shortenPrincipal(p), type: 'canister', linkTarget: p });
  }

  const callerText = event.caller?.toText?.() ?? (typeof event.caller === 'string' ? event.caller : null);
  if (callerText) fields.push({ label: 'Caller', value: shortenPrincipal(callerText), type: 'address', linkTarget: callerText });

  if (event.timestamp) fields.push({ label: 'Timestamp', value: formatTimestamp(event.timestamp), type: 'timestamp', linkTarget: String(event.timestamp) });

  return {
    summary,
    typeName: '3Pool Admin',
    category: 'admin',
    badgeColor: BADGE_COLORS.admin,
    fields,
  };
}

/**
 * Format a Stability Pool PoolEvent into FormattedEvent.
 * PoolEvent: { id, timestamp, caller, event_type: Deposit|Withdraw|ClaimCollateral|DepositAs3USD|InterestReceived }
 */
export function formatStabilityPoolEvent(evt: any): FormattedEvent {
  const eventType = evt.event_type ?? {};
  const key = Object.keys(eventType)[0] ?? 'unknown';
  const data = eventType[key] ?? {};

  const fields: EventField[] = [];
  let summary = '';
  let typeName = '';

  if (key === 'Deposit') {
    const sym = getTokenSymbol(data.token_ledger?.toText?.() ?? '');
    const dec = getTokenDecimals(data.token_ledger?.toText?.() ?? '');
    const amt = formatE8s(data.amount, dec);
    typeName = 'Deposit';
    summary = `Deposited ${amt} ${sym}`;
    fields.push({ label: 'Amount', value: `${amt} ${sym}`, type: 'amount' });
  } else if (key === 'Withdraw') {
    const sym = getTokenSymbol(data.token_ledger?.toText?.() ?? '');
    const dec = getTokenDecimals(data.token_ledger?.toText?.() ?? '');
    const amt = formatE8s(data.amount, dec);
    typeName = 'Withdraw';
    summary = `Withdrew ${amt} ${sym}`;
    fields.push({ label: 'Amount', value: `${amt} ${sym}`, type: 'amount' });
  } else if (key === 'ClaimCollateral') {
    const sym = getTokenSymbol(data.collateral_ledger?.toText?.() ?? '');
    const dec = getTokenDecimals(data.collateral_ledger?.toText?.() ?? '');
    const amt = formatE8s(data.amount, dec);
    typeName = 'Claim Collateral';
    summary = `Claimed ${amt} ${sym}`;
    fields.push({ label: 'Amount', value: `${amt} ${sym}`, type: 'amount' });
  } else if (key === 'DepositAs3USD') {
    const sym = getTokenSymbol(data.token_ledger?.toText?.() ?? '');
    const dec = getTokenDecimals(data.token_ledger?.toText?.() ?? '');
    const amtIn = formatE8s(data.amount_in, dec);
    const lpMinted = formatE8s(data.lp_minted, 8);
    typeName = 'Deposit as 3USD';
    summary = `Deposited ${amtIn} ${sym} → ${lpMinted} 3USD LP`;
    fields.push({ label: 'Amount In', value: `${amtIn} ${sym}`, type: 'amount' });
    fields.push({ label: 'LP Minted', value: `${lpMinted} 3USD`, type: 'amount' });
  } else if (key === 'InterestReceived') {
    const sym = getTokenSymbol(data.token_ledger?.toText?.() ?? '');
    const dec = getTokenDecimals(data.token_ledger?.toText?.() ?? '');
    const amt = formatE8s(data.amount, dec);
    typeName = 'Interest Received';
    summary = `Received ${amt} ${sym} interest`;
    fields.push({ label: 'Amount', value: `${amt} ${sym}`, type: 'amount' });
  } else if (key === 'OptOutCollateral') {
    const sym = getTokenSymbol(data.collateral_type?.toText?.() ?? '');
    typeName = 'Opt Out Collateral';
    summary = `Opted out of ${sym} collateral`;
  } else if (key === 'OptInCollateral') {
    const sym = getTokenSymbol(data.collateral_type?.toText?.() ?? '');
    typeName = 'Opt In Collateral';
    summary = `Opted in to ${sym} collateral`;
  } else if (key === 'LiquidationNotification') {
    typeName = 'Liquidation Notification';
    summary = `Liquidation notification: ${data.vault_count} vaults`;
  } else if (key === 'LiquidationExecuted') {
    const sym = getTokenSymbol(data.collateral_type?.toText?.() ?? '');
    const collAmt = formatE8s(data.collateral_gained, 8);
    const stables = formatE8s(data.stables_consumed_e8s, 8);
    typeName = data.success ? 'Liquidation Executed' : 'Liquidation Failed';
    summary = data.success
      ? `Liquidated vault #${data.vault_id}: consumed ${stables} stables, gained ${collAmt} ${sym}`
      : `Liquidation failed for vault #${data.vault_id}`;
    fields.push({ label: 'Vault', value: `#${data.vault_id}`, type: 'vault', linkTarget: String(data.vault_id) });
    fields.push({ label: 'Stables Consumed', value: `${stables}`, type: 'amount' });
    fields.push({ label: 'Collateral Gained', value: `${collAmt} ${sym}`, type: 'amount' });
  } else if (key === 'StablecoinRegistered') {
    typeName = 'Stablecoin Registered';
    summary = `Registered stablecoin: ${data.symbol}`;
  } else if (key === 'CollateralRegistered') {
    typeName = 'Collateral Registered';
    summary = `Registered collateral: ${data.symbol}`;
  } else if (key === 'ConfigurationUpdated') {
    typeName = 'Config Updated';
    summary = 'Pool configuration updated';
  } else if (key === 'EmergencyPauseActivated') {
    typeName = 'Emergency Pause';
    summary = 'Emergency pause activated';
  } else if (key === 'OperationsResumed') {
    typeName = 'Operations Resumed';
    summary = 'Operations resumed';
  } else if (key === 'BalanceCorrected') {
    const tokenId = data.token_ledger?.toText?.() ?? '';
    const sym = getTokenSymbol(tokenId);
    const dec = getTokenDecimals(tokenId);
    const amt = formatE8s(data.new_amount, dec);
    typeName = 'Balance Corrected';
    summary = `Balance corrected for ${shortenPrincipal(data.user?.toText?.() ?? '')}: ${amt} ${sym}`;
  } else if (key === 'CollateralGainCorrected') {
    const collId = data.collateral_ledger?.toText?.() ?? '';
    const sym = getTokenSymbol(collId);
    const dec = getTokenDecimals(collId);
    const amt = formatE8s(data.new_amount, dec);
    typeName = 'Collateral Gain Corrected';
    summary = `Collateral gain corrected for ${shortenPrincipal(data.user?.toText?.() ?? '')}: ${amt} ${sym}`;
  } else {
    typeName = key;
    summary = key;
  }

  const callerText = evt.caller?.toText?.() ?? evt.caller?.toString?.() ?? null;
  if (callerText) {
    fields.push({
      label: 'Caller',
      value: shortenPrincipal(callerText),
      type: 'address',
      linkTarget: callerText,
    });
  }

  if (evt.timestamp) {
    fields.push({ label: 'Timestamp', value: formatTimestamp(evt.timestamp), type: 'timestamp' });
  }

  return {
    summary,
    typeName,
    category: 'stability_pool',
    badgeColor: BADGE_COLORS.stability_pool,
    fields,
  };
}

// ─── Helpers ──────────────────────────────────────────────────────────

function getVariantKey(event: any): string {
  const eventType = event.event_type ?? event;
  return Object.keys(eventType)[0] ?? 'unknown';
}

function getVariantData(event: any): any {
  const eventType = event.event_type ?? event;
  const key = Object.keys(eventType)[0];
  return key ? eventType[key] : {};
}

function principalToText(p: any): string {
  if (!p) return '';
  return p?.toText?.() ?? p?.toString?.() ?? String(p);
}

function optPrincipalToText(p: any): string | null {
  if (Array.isArray(p)) {
    return p.length > 0 ? principalToText(p[0]) : null;
  }
  if (p) return principalToText(p);
  return null;
}

function optValue<T>(v: any): T | undefined {
  if (Array.isArray(v)) return v.length > 0 ? v[0] : undefined;
  return v ?? undefined;
}

function fmtE8s(e8s: any, decimals = 8): string {
  if (e8s === undefined || e8s === null) return '0';
  return formatE8s(e8s, decimals);
}

function tokenSymbol(principal: any): string {
  return getTokenSymbol(principalToText(principal));
}

function tokenDecimals(principal: any): number {
  return getTokenDecimals(principalToText(principal));
}

// ─── Field Builders ───────────────────────────────────────────────────

function vaultField(vaultId: any): EventField {
  const id = String(Number(vaultId));
  return { label: 'Vault', value: `#${id}`, type: 'vault', linkTarget: id };
}

function addressField(label: string, principal: any): EventField | null {
  const text = optPrincipalToText(principal) ?? principalToText(principal);
  if (!text) return null;
  const name = getCanisterName(text);
  return {
    label,
    value: name ?? shortenPrincipal(text),
    type: 'address',
    linkTarget: text,
  };
}

function tokenField(principal: any): EventField {
  const text = principalToText(principal);
  return {
    label: 'Collateral',
    value: tokenSymbol(principal),
    type: 'token',
    linkTarget: text,
    tokenPrincipal: text,
  };
}

function amountField(label: string, e8s: any, decimals = 8, symbol = 'icUSD'): EventField {
  return { label, value: `${fmtE8s(e8s, decimals)} ${symbol}`, type: 'amount' };
}

function tokenAmountField(label: string, e8s: any, tokenPrincipal: any): EventField {
  const text = principalToText(tokenPrincipal);
  const sym = tokenSymbol(tokenPrincipal);
  const dec = tokenDecimals(tokenPrincipal);
  return {
    label,
    value: `${fmtE8s(e8s, dec)} ${sym}`,
    type: 'amount',
    tokenPrincipal: text,
  };
}

function usdField(label: string, e8s: any): EventField {
  const val = Number(e8s) / 1e8;
  return {
    label,
    value: `$${val.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`,
    type: 'usd',
  };
}

function textField(label: string, value: any): EventField {
  return { label, value: String(value), type: 'text' };
}

function percentField(label: string, value: any): EventField {
  return { label, value: String(value), type: 'percentage' };
}

function ratioField(label: string, value: any): EventField {
  return { label, value: String(value), type: 'ratio' };
}

function blockIndexField(label: string, index: any): EventField {
  return { label, value: String(Number(index)), type: 'block_index' };
}

function timestampField(ns: any): EventField {
  return { label: 'Timestamp', value: formatTimestamp(ns), type: 'timestamp' };
}

function jsonField(label: string, value: any): EventField {
  let str: string;
  try {
    str = JSON.stringify(value, (_k, v) => (typeof v === 'bigint' ? v.toString() : v), 2);
  } catch {
    str = String(value);
  }
  return { label, value: str, type: 'json' };
}

function canisterField(label: string, principal: any): EventField {
  const text = principalToText(principal);
  const name = getCanisterName(text);
  return {
    label,
    value: name ?? shortenPrincipal(text),
    type: 'canister',
    linkTarget: text,
  };
}

function pushIfPresent(fields: EventField[], field: EventField | null) {
  if (field) fields.push(field);
}

// ─── Category Resolution ──────────────────────────────────────────────

export function getEventCategory(event: any): EventCategory {
  const key = getVariantKey(event);
  if (VAULT_OPS_KEYS.has(key)) return 'vault_ops';
  if (LIQUIDATION_KEYS.has(key)) return 'liquidation';
  if (REDEMPTION_KEYS.has(key)) return 'redemption';
  if (STABILITY_POOL_KEYS.has(key)) return 'stability_pool';
  if (SYSTEM_KEYS.has(key)) return 'system';
  // Admin is the catch-all for Set*, Add*, Update*, Admin* events
  return 'admin';
}

// ─── Type Name Lookup ─────────────────────────────────────────────────

const TYPE_NAMES: Record<string, string> = {
  open_vault: 'Open Vault',
  close_vault: 'Close Vault',
  withdraw_and_close_vault: 'Withdraw & Close',
  vault_withdrawn_and_closed: 'Withdraw & Close',
  VaultWithdrawnAndClosed: 'Withdraw & Close',
  borrow_from_vault: 'Borrow',
  repay_to_vault: 'Repay',
  dust_forgiven: 'Dust Forgiven',
  add_margin_to_vault: 'Add Collateral',
  collateral_withdrawn: 'Withdraw All Collateral',
  partial_collateral_withdrawn: 'Withdraw Collateral',
  margin_transfer: 'Margin Transfer',
  liquidate_vault: 'Full Liquidation',
  partial_liquidate_vault: 'Partial Liquidation',
  redistribute_vault: 'Redistribution',
  bot_liquidation_claimed: 'Bot Liquidation Claimed',
  bot_liquidation_confirmed: 'Bot Liquidation Confirmed',
  bot_liquidation_canceled: 'Bot Liquidation Canceled',
  provide_liquidity: 'Deposit to Stability Pool',
  withdraw_liquidity: 'Withdraw from Stability Pool',
  claim_liquidity_returns: 'Claim SP Returns',
  redemption_on_vaults: 'Redemption',
  redemption_transfered: 'Redemption Transfer',
  reserve_redemption: 'Reserve Redemption',
  add_collateral_type: 'Add Collateral Type',
  update_collateral_status: 'Update Collateral Status',
  update_collateral_config: 'Update Collateral Config',
  set_borrowing_fee: 'Set Borrowing Fee',
  set_collateral_borrowing_fee: 'Set Collateral Borrowing Fee',
  set_liquidation_bonus: 'Set Liquidation Bonus',
  set_interest_rate: 'Set Interest Rate',
  set_interest_split: 'Set Interest Split',
  set_interest_pool_share: 'Set Interest Pool Share',
  set_redemption_fee_floor: 'Set Redemption Fee Floor',
  set_redemption_fee_ceiling: 'Set Redemption Fee Ceiling',
  set_ckstable_repay_fee: 'Set ckStable Repay Fee',
  set_min_icusd_amount: 'Set Min icUSD Amount',
  set_global_icusd_mint_cap: 'Set Global icUSD Mint Cap',
  set_max_partial_liquidation_ratio: 'Set Max Partial Liquidation Ratio',
  set_recovery_target_cr: 'Set Recovery Target CR',
  set_recovery_cr_multiplier: 'Set Recovery CR Multiplier',
  set_liquidation_protocol_share: 'Set Liquidation Protocol Share',
  set_reserve_redemptions_enabled: 'Set Reserve Redemptions',
  set_reserve_redemption_fee: 'Set Reserve Redemption Fee',
  set_rate_curve_markers: 'Set Rate Curve Markers',
  set_recovery_rate_curve: 'Set Recovery Rate Curve',
  set_borrowing_fee_curve: 'Set Borrowing Fee Curve',
  set_rmr_floor: 'Set RMR Floor',
  set_rmr_ceiling: 'Set RMR Ceiling',
  set_rmr_floor_cr: 'Set RMR Floor CR',
  set_rmr_ceiling_cr: 'Set RMR Ceiling CR',
  set_stable_ledger_principal: 'Set Stable Ledger Principal',
  set_treasury_principal: 'Set Treasury Principal',
  set_stability_pool_principal: 'Set Stability Pool Principal',
  set_liquidation_bot_principal: 'Set Liquidation Bot Principal',
  set_three_pool_canister: 'Set 3Pool Canister',
  set_bot_budget: 'Set Bot Budget',
  set_bot_allowed_collateral_types: 'Set Bot Collateral Types',
  set_stable_token_enabled: 'Set Stable Token Enabled',
  set_healthy_cr: 'Set Healthy CR',
  set_recovery_parameters: 'Set Recovery Parameters',
  admin_vault_correction: 'Admin Vault Correction',
  admin_mint: 'Admin Mint',
  admin_sweep_to_treasury: 'Admin Sweep to Treasury',
  init: 'Protocol Init',
  upgrade: 'Protocol Upgrade',
  accrue_interest: 'Accrue Interest',
};

function getTypeName(key: string): string {
  return TYPE_NAMES[key] ?? key.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

// ─── Main Format Function ─────────────────────────────────────────────

/**
 * Format a backend protocol event for display.
 * @param event - Raw event from the backend canister
 * @param vaultCollateralMap - Optional map of vault_id → collateral_type principal string.
 *   Used to look up the correct collateral token for events that don't include it
 *   (e.g. partial_collateral_withdrawn, add_margin_to_vault, collateral_withdrawn).
 */
export function formatEvent(event: any, vaultCollateralMap?: Map<number, string>): FormattedEvent {
  const key = getVariantKey(event);
  const d = getVariantData(event);
  const category = getEventCategory(event);
  const typeName = getTypeName(key);
  const badgeColor = BADGE_COLORS[category];
  const fields: EventField[] = [];

  // Timestamp helper — appended at the end for most events
  const ts = optValue<any>(d?.timestamp);

  // Look up vault collateral type from the map when the event doesn't include it
  function vaultCollateral(vaultId: any): string {
    if (vaultId == null) return 'unknown';
    return vaultCollateralMap?.get(Number(vaultId)) ?? 'unknown';
  }

  switch (key) {
    // ── Vault Lifecycle ─────────────────────────────────────────────

    case 'open_vault': {
      const vault = d.vault;
      if (vault) {
        const sym = tokenSymbol(vault.collateral_type);
        const dec = tokenDecimals(vault.collateral_type);
        const collAmt = fmtE8s(vault.collateral_amount, dec);
        const debtAmt = fmtE8s(vault.borrowed_icusd_amount);
        fields.push(vaultField(vault.vault_id));
        pushIfPresent(fields, addressField('Owner', vault.owner));
        fields.push(tokenField(vault.collateral_type));
        fields.push(tokenAmountField('Collateral Deposited', vault.collateral_amount, vault.collateral_type));
        fields.push(amountField('Debt', vault.borrowed_icusd_amount));
        if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
        if (ts) fields.push(timestampField(ts));
        return {
          summary: `Vault #${vault.vault_id} opened with ${collAmt} ${sym}, borrowed ${debtAmt} icUSD`,
          typeName, category, badgeColor, fields,
        };
      }
      break;
    }

    case 'close_vault': {
      fields.push(vaultField(d.vault_id));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', optValue(d.block_index)));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Vault #${d.vault_id} closed`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'withdraw_and_close_vault':
    case 'vault_withdrawn_and_closed':
    case 'VaultWithdrawnAndClosed': {
      const sym = tokenSymbol(d.collateral_type ?? 'unknown');
      fields.push(vaultField(d.vault_id));
      if (d.amount !== undefined) fields.push(amountField('Collateral Returned', d.amount, 8, sym || 'ICP'));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', optValue(d.block_index)));
      const eventTs = optValue<any>(d.timestamp) ?? ts;
      if (eventTs) fields.push(timestampField(eventTs));
      return {
        summary: `Withdrew and closed Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Borrowing ───────────────────────────────────────────────────

    case 'borrow_from_vault': {
      const amt = fmtE8s(d.borrowed_amount);
      const fee = fmtE8s(d.fee_amount);
      fields.push(vaultField(d.vault_id));
      fields.push(amountField('Borrowed', d.borrowed_amount));
      fields.push(amountField('Borrowing Fee', d.fee_amount));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Borrowed ${amt} icUSD from Vault #${d.vault_id} (fee: ${fee} icUSD)`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'repay_to_vault': {
      const amt = fmtE8s(d.repayed_amount);
      fields.push(vaultField(d.vault_id));
      fields.push(amountField('Repaid', d.repayed_amount));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Repaid ${amt} icUSD to Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'dust_forgiven': {
      const amt = fmtE8s(d.amount);
      fields.push(vaultField(d.vault_id));
      fields.push(amountField('Dust Amount', d.amount));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Forgave ${amt} icUSD dust on Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Collateral ──────────────────────────────────────────────────

    case 'add_margin_to_vault': {
      const collType = d.collateral_type ?? vaultCollateral(d.vault_id);
      const sym = tokenSymbol(collType);
      const dec = tokenDecimals(collType);
      const amt = fmtE8s(d.margin_added, dec);
      fields.push(vaultField(d.vault_id));
      if (collType !== 'unknown') fields.push(tokenField(collType));
      fields.push(tokenAmountField('Collateral Added', d.margin_added, collType));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Added ${amt} ${sym} to Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'collateral_withdrawn': {
      const collType = vaultCollateral(d.vault_id);
      const sym = tokenSymbol(collType);
      const dec = tokenDecimals(collType);
      const amt = fmtE8s(d.amount, dec);
      fields.push(vaultField(d.vault_id));
      if (collType !== 'unknown') fields.push(tokenField(collType));
      fields.push(amountField('Amount', d.amount, dec, sym));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Withdrew all collateral from Vault #${d.vault_id} (${amt} ${sym})`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'partial_collateral_withdrawn': {
      const collType = vaultCollateral(d.vault_id);
      const sym = tokenSymbol(collType);
      const dec = tokenDecimals(collType);
      const amt = fmtE8s(d.amount, dec);
      fields.push(vaultField(d.vault_id));
      if (collType !== 'unknown') fields.push(tokenField(collType));
      fields.push(amountField('Amount Withdrawn', d.amount, dec, sym));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Withdrew ${amt} ${sym} from Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'margin_transfer': {
      fields.push(vaultField(d.vault_id));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Margin transfer completed for Vault #${d.vault_id}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Liquidations ────────────────────────────────────────────────

    case 'liquidate_vault': {
      fields.push(vaultField(d.vault_id));
      if (d.mode) {
        const modeKey = typeof d.mode === 'object' ? Object.keys(d.mode)[0] : String(d.mode);
        fields.push(textField('Mode', modeKey));
      }
      if (d.icp_rate !== undefined) fields.push(textField('ICP Rate', `$${Number(d.icp_rate).toFixed(4)}`));
      pushIfPresent(fields, addressField('Liquidator', d.liquidator));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Vault #${d.vault_id} fully liquidated`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'partial_liquidate_vault': {
      const payment = fmtE8s(d.liquidator_payment);
      const collateral = fmtE8s(d.icp_to_liquidator);
      fields.push(vaultField(d.vault_id));
      fields.push(amountField('Debt Repaid', d.liquidator_payment));
      fields.push(amountField('Collateral to Liquidator', d.icp_to_liquidator, 8, 'ICP'));
      if (d.protocol_fee_collateral !== undefined) {
        const fee = optValue(d.protocol_fee_collateral);
        if (fee !== undefined) fields.push(amountField('Protocol Fee', fee, 8, 'ICP'));
      }
      pushIfPresent(fields, addressField('Liquidator', d.liquidator));
      if (d.icp_rate !== undefined) {
        const rate = optValue(d.icp_rate);
        if (rate !== undefined) fields.push(textField('ICP Rate', `$${Number(rate).toFixed(4)}`));
      }
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Vault #${d.vault_id} partially liquidated — ${payment} icUSD debt repaid, ${collateral} ICP seized`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'redistribute_vault': {
      fields.push(vaultField(d.vault_id));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Vault #${d.vault_id} debt and collateral redistributed`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'bot_liquidation_claimed': {
      if (d.vault_id !== undefined) fields.push(vaultField(d.vault_id));
      pushIfPresent(fields, addressField('Bot', d.bot));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Bot claimed liquidation on Vault #${d.vault_id ?? '?'}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'bot_liquidation_confirmed': {
      if (d.vault_id !== undefined) fields.push(vaultField(d.vault_id));
      pushIfPresent(fields, addressField('Bot', d.bot));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Bot liquidation confirmed on Vault #${d.vault_id ?? '?'}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'bot_liquidation_canceled': {
      if (d.vault_id !== undefined) fields.push(vaultField(d.vault_id));
      pushIfPresent(fields, addressField('Bot', d.bot));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Bot liquidation canceled on Vault #${d.vault_id ?? '?'}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Stability Pool ──────────────────────────────────────────────

    case 'provide_liquidity': {
      const amt = fmtE8s(d.amount);
      fields.push(amountField('Deposited', d.amount));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Deposited ${amt} icUSD to Stability Pool`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'withdraw_liquidity': {
      const amt = fmtE8s(d.amount);
      fields.push(amountField('Withdrawn', d.amount));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Withdrew ${amt} icUSD from Stability Pool`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'claim_liquidity_returns': {
      const amt = fmtE8s(d.amount);
      fields.push(amountField('Claimed', d.amount, 8, 'ICP'));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Claimed ${amt} ICP from Stability Pool`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Redemptions ─────────────────────────────────────────────────

    case 'redemption_on_vaults': {
      const amt = fmtE8s(d.icusd_amount);
      const fee = fmtE8s(d.fee_amount);
      pushIfPresent(fields, addressField('Redeemer', d.owner));
      fields.push(amountField('icUSD Redeemed', d.icusd_amount));
      fields.push(amountField('Fee', d.fee_amount));
      if (d.current_icp_rate !== undefined) {
        fields.push(textField('ICP Rate', `$${Number(d.current_icp_rate).toFixed(4)}`));
      }
      if (d.icusd_block_index !== undefined) fields.push(blockIndexField('icUSD Block Index', d.icusd_block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Redeemed ${amt} icUSD (fee: ${fee} icUSD)`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'redemption_transfered': {
      if (d.icusd_block_index !== undefined) fields.push(blockIndexField('icUSD Block Index', d.icusd_block_index));
      if (d.icp_block_index !== undefined) fields.push(blockIndexField('ICP Block Index', d.icp_block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Redemption collateral transferred (ICP block #${d.icp_block_index ?? '?'})`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'reserve_redemption': {
      const amt = fmtE8s(d.icusd_amount);
      const fee = fmtE8s(d.fee_amount);
      pushIfPresent(fields, addressField('Redeemer', d.owner));
      fields.push(amountField('icUSD Redeemed', d.icusd_amount));
      fields.push(amountField('Fee', d.fee_amount));
      if (d.stable_token_ledger) {
        const sym = tokenSymbol(d.stable_token_ledger);
        fields.push(canisterField('Stable Token', d.stable_token_ledger));
        if (d.stable_amount_sent !== undefined) {
          const stDec = tokenDecimals(d.stable_token_ledger);
          fields.push(amountField('Stable Sent', d.stable_amount_sent, stDec, sym));
        }
        if (d.fee_stable_amount !== undefined) {
          const stDec = tokenDecimals(d.stable_token_ledger);
          fields.push(amountField('Stable Fee', d.fee_stable_amount, stDec, sym));
        }
      }
      if (d.icusd_block_index !== undefined) fields.push(blockIndexField('icUSD Block Index', d.icusd_block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Reserve redemption: ${amt} icUSD (fee: ${fee} icUSD)`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Collateral ────────────────────────────────────────────

    case 'add_collateral_type': {
      const sym = tokenSymbol(d.collateral_type);
      fields.push(tokenField(d.collateral_type));
      if (d.config) fields.push(jsonField('Config', d.config));
      return {
        summary: `Added new collateral type: ${sym}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'update_collateral_status': {
      const sym = tokenSymbol(d.collateral_type);
      fields.push(tokenField(d.collateral_type));
      let statusName = 'Unknown';
      if (d.status && typeof d.status === 'object') {
        statusName = Object.keys(d.status).find((k) => d.status[k] === null) ?? 'Unknown';
      }
      fields.push(textField('New Status', statusName));
      return {
        summary: `Set ${sym} status to ${statusName}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'update_collateral_config': {
      const sym = tokenSymbol(d.collateral_type);
      fields.push(tokenField(d.collateral_type));
      if (d.config) fields.push(jsonField('Config', d.config));
      return {
        summary: `Updated ${sym} collateral configuration`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Fees / Rates (simple {rate: string} pattern) ──────────

    case 'set_borrowing_fee':
    case 'set_liquidation_bonus':
    case 'set_redemption_fee_floor':
    case 'set_redemption_fee_ceiling':
    case 'set_ckstable_repay_fee':
    case 'set_max_partial_liquidation_ratio':
    case 'set_recovery_target_cr':
    case 'set_reserve_redemption_fee': {
      const val = d.rate ?? d.fee ?? d.value ?? '?';
      fields.push(percentField('New Value', val));
      return {
        summary: `${typeName}: ${val}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_recovery_cr_multiplier': {
      const val = d.multiplier ?? d.buffer ?? '?';
      fields.push(ratioField('Multiplier', val));
      return {
        summary: `Set recovery CR multiplier to ${val}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_liquidation_protocol_share': {
      fields.push(percentField('Protocol Share', d.share));
      return {
        summary: `Set liquidation protocol share to ${d.share}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_interest_pool_share': {
      fields.push(percentField('Pool Share', d.share));
      return {
        summary: `Set interest pool share to ${d.share}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_min_icusd_amount': {
      fields.push(textField('Min Amount', d.amount));
      return {
        summary: `Set min icUSD amount to ${d.amount}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_global_icusd_mint_cap': {
      const val = d.amount ?? d.cap ?? '?';
      fields.push(textField('Mint Cap', val));
      return {
        summary: `Set global icUSD mint cap to ${val}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Per-Collateral Settings ───────────────────────────────

    case 'set_collateral_borrowing_fee': {
      const sym = tokenSymbol(d.collateral_type);
      const val = d.borrowing_fee ?? d.rate ?? d.fee ?? '?';
      fields.push(tokenField(d.collateral_type));
      fields.push(percentField('Borrowing Fee', val));
      return {
        summary: `Set ${sym} borrowing fee to ${val}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_interest_rate': {
      const sym = tokenSymbol(d.collateral_type);
      fields.push(tokenField(d.collateral_type));
      fields.push(percentField('Interest Rate APR', d.interest_rate_apr));
      return {
        summary: `Set ${sym} interest rate to ${d.interest_rate_apr}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_interest_split': {
      fields.push(jsonField('Split', d.split));
      return {
        summary: 'Updated interest revenue split',
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_healthy_cr': {
      const ct = d.collateral_type ?? '?';
      const val = optValue(d.healthy_cr) ?? 'removed';
      fields.push(textField('Collateral Type', ct));
      fields.push(ratioField('Healthy CR', val));
      return {
        summary: `Set healthy CR for ${ct} to ${val}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_recovery_parameters': {
      const sym = tokenSymbol(d.collateral_type);
      fields.push(tokenField(d.collateral_type));
      if (d.recovery_borrowing_fee) {
        const v = optValue(d.recovery_borrowing_fee);
        if (v) fields.push(percentField('Recovery Borrowing Fee', v));
      }
      if (d.recovery_interest_rate_apr) {
        const v = optValue(d.recovery_interest_rate_apr);
        if (v) fields.push(percentField('Recovery Interest Rate APR', v));
      }
      return {
        summary: `Updated recovery parameters for ${sym}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Rate Curves ───────────────────────────────────────────

    case 'set_rate_curve_markers': {
      const ct = optValue(d.collateral_type);
      if (ct) fields.push(textField('Collateral Type', ct));
      fields.push(jsonField('Markers', d.markers));
      return {
        summary: ct ? `Set rate curve markers for ${ct}` : 'Set global rate curve markers',
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_recovery_rate_curve': {
      fields.push(jsonField('Markers', d.markers));
      return {
        summary: 'Updated recovery rate curve',
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_borrowing_fee_curve': {
      fields.push(jsonField('Markers', d.markers));
      return {
        summary: 'Updated borrowing fee curve',
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin RMR ───────────────────────────────────────────────────

    case 'set_rmr_floor':
    case 'set_rmr_ceiling':
    case 'set_rmr_floor_cr':
    case 'set_rmr_ceiling_cr': {
      fields.push(ratioField('Value', d.value));
      return {
        summary: `${typeName}: ${d.value}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Principals ────────────────────────────────────────────

    case 'set_stable_ledger_principal': {
      if (d.token_type) {
        const tokenTypeKey = typeof d.token_type === 'object' ? Object.keys(d.token_type)[0] : String(d.token_type);
        fields.push(textField('Token Type', tokenTypeKey));
      }
      fields.push(canisterField('Principal', d.principal));
      return {
        summary: `Set stable ledger principal to ${shortenPrincipal(principalToText(d.principal))}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_treasury_principal': {
      fields.push(canisterField('Treasury', d.principal));
      return {
        summary: `Set treasury principal to ${shortenPrincipal(principalToText(d.principal))}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_stability_pool_principal': {
      fields.push(canisterField('Stability Pool', d.principal));
      return {
        summary: `Set stability pool principal to ${shortenPrincipal(principalToText(d.principal))}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_liquidation_bot_principal': {
      fields.push(canisterField('Liquidation Bot', d.principal));
      return {
        summary: `Set liquidation bot principal to ${shortenPrincipal(principalToText(d.principal))}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_three_pool_canister': {
      fields.push(canisterField('3Pool Canister', d.canister));
      return {
        summary: `Set 3Pool canister to ${shortenPrincipal(principalToText(d.canister))}`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Other ─────────────────────────────────────────────────

    case 'set_bot_budget': {
      const budget = fmtE8s(d.total_e8s);
      fields.push(amountField('Budget', d.total_e8s, 8, 'icUSD'));
      if (d.start_timestamp) fields.push(timestampField(d.start_timestamp));
      return {
        summary: `Set bot budget to ${budget} icUSD`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_bot_allowed_collateral_types': {
      const types = (d.collateral_types ?? []).map((p: any) => tokenSymbol(p));
      fields.push(textField('Collateral Types', types.join(', ')));
      return {
        summary: `Set bot allowed collateral types: ${types.join(', ')}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_stable_token_enabled': {
      const tokenTypeKey = typeof d.token_type === 'object' ? Object.keys(d.token_type)[0] : String(d.token_type);
      fields.push(textField('Token Type', tokenTypeKey));
      fields.push(textField('Enabled', String(d.enabled)));
      return {
        summary: `${d.enabled ? 'Enabled' : 'Disabled'} stable token: ${tokenTypeKey}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'set_reserve_redemptions_enabled': {
      fields.push(textField('Enabled', String(d.enabled)));
      return {
        summary: `${d.enabled ? 'Enabled' : 'Disabled'} reserve redemptions`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── Admin Corrections ───────────────────────────────────────────

    case 'admin_vault_correction': {
      const oldAmt = fmtE8s(d.old_amount);
      const newAmt = fmtE8s(d.new_amount);
      fields.push(vaultField(d.vault_id));
      fields.push(amountField('Old Amount', d.old_amount, 8, 'e8s'));
      fields.push(amountField('New Amount', d.new_amount, 8, 'e8s'));
      if (d.reason) fields.push(textField('Reason', d.reason));
      return {
        summary: `Admin corrected Vault #${d.vault_id} collateral: ${oldAmt} -> ${newAmt}`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'admin_mint': {
      const amt = fmtE8s(d.amount);
      fields.push(amountField('Minted', d.amount));
      pushIfPresent(fields, addressField('To', d.to));
      if (d.reason) fields.push(textField('Reason', d.reason));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: `Admin minted ${amt} icUSD`,
        typeName, category, badgeColor, fields,
      };
    }

    case 'admin_sweep_to_treasury': {
      const amt = fmtE8s(d.amount);
      fields.push(amountField('Amount Swept', d.amount, 8, 'e8s'));
      pushIfPresent(fields, addressField('Treasury', d.treasury));
      if (d.block_index !== undefined) fields.push(blockIndexField('Block Index', d.block_index));
      if (d.reason) fields.push(textField('Reason', d.reason));
      return {
        summary: `Swept ${amt} to treasury`,
        typeName, category, badgeColor, fields,
      };
    }

    // ── System ──────────────────────────────────────────────────────

    case 'init': {
      // d is the InitArg struct
      fields.push(jsonField('Init Args', d));
      return {
        summary: 'Protocol initialized',
        typeName, category, badgeColor, fields,
      };
    }

    case 'upgrade': {
      // d is the UpgradeArg struct
      const desc = optValue<string>(d?.description);
      if (desc) fields.push(textField('Description', desc));
      const mode = optValue(d?.mode);
      if (mode) {
        const modeKey = typeof mode === 'object' ? Object.keys(mode)[0] : String(mode);
        fields.push(textField('Mode', modeKey));
      }
      return {
        summary: desc ? `Protocol upgraded — ${desc}` : 'Protocol upgraded',
        typeName, category, badgeColor, fields,
      };
    }

    case 'accrue_interest': {
      if (d.timestamp) fields.push(timestampField(d.timestamp));
      return {
        summary: 'Interest accrued across all vaults',
        typeName, category, badgeColor, fields,
      };
    }

    // ── Default / Fallback ──────────────────────────────────────────

    default: {
      // Render unrecognized events with raw data
      if (d.vault_id !== undefined) fields.push(vaultField(d.vault_id));
      if (d.vault?.vault_id !== undefined) fields.push(vaultField(d.vault.vault_id));
      pushIfPresent(fields, addressField('Caller', d.caller));
      if (d.owner) pushIfPresent(fields, addressField('Owner', d.owner));
      fields.push(jsonField('Data', d));
      if (ts) fields.push(timestampField(ts));
      return {
        summary: typeName,
        typeName, category, badgeColor, fields,
      };
    }
  }

  // Fallback for any case that didn't return (e.g. open_vault with no vault data)
  if (ts) fields.push(timestampField(ts));
  return {
    summary: typeName,
    typeName,
    category,
    badgeColor,
    fields,
  };
}
