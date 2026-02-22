use crate::numeric::{ICUSD, ICP};
use crate::state::read_state;
use crate::StableTokenType;
use candid::{Nat, Principal};
use ic_xrc_types::{Asset, AssetClass, GetExchangeRateRequest, GetExchangeRateResult};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_client_cdk::{CdkRuntime, ICRC1Client};
use num_traits::ToPrimitive;
use std::fmt;
use crate::log;
use crate::DEBUG;

/// Represents an error from a management canister call
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    method: String,
    reason: Reason,
}

impl CallError {
    /// Returns the name of the method that resulted in this error.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the failure reason.
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "management call '{}' failed: {}",
            self.method, self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The reason for the management call failure.
pub enum Reason {
    /// Failed to send a signature request because the local output queue is
    /// full.
    QueueIsFull,
    /// The canister does not have enough cycles to submit the request.
    OutOfCycles,
    /// The call failed with an error.
    CanisterError(String),
    /// The management canister rejected the signature request (not enough
    /// cycles, the ECDSA subnet is overloaded, etc.).
    Rejected(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueIsFull => write!(fmt, "the canister queue is full"),
            Self::OutOfCycles => write!(fmt, "the canister is out of cycles"),
            Self::CanisterError(msg) => write!(fmt, "canister error: {}", msg),
            Self::Rejected(msg) => {
                write!(fmt, "the management canister rejected the call: {}", msg)
            }
        }
    }
}

/// Query the XRC canister to retrieve the last BTC/USD price.
/// https://github.com/dfinity/exchange-rate-canister
pub async fn fetch_icp_price() -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let icp = Asset {
        symbol: "ICP".to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(), 
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: icp,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),  // Clone args for logging
        XRC_CALL_COST_CYCLES,
    )
    .await;

    // Add detailed logging
    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC error code: {:?}, message: {}", code, msg);  // Changed to {:?}
            Err(format!(
                "Error while calling XRC canister ({:?}): {:?}",  // Changed to {:?}
                code, msg
            ))
        }
    }
}

/// Fetch USDT/USD or USDC/USD price from the XRC canister.
/// Used on-demand for depeg protection on ckstable operations.
pub async fn fetch_stable_price(symbol: &str) -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let stable = Asset {
        symbol: symbol.to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(),
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: stable,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),
        XRC_CALL_COST_CYCLES,
    )
    .await;

    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_stable_price] XRC request for {}: {:?}", symbol, args);
            log!(DEBUG, "[fetch_stable_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_stable_price] XRC error for {}: {:?}, message: {}", symbol, code, msg);
            Err(format!(
                "Error fetching {} price from XRC ({:?}): {:?}",
                symbol, code, msg
            ))
        }
    }
}

pub async fn mint_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: read_state(|s| s.icusd_ledger_principal),
    };
    let block_index = client
        .transfer(TransferArg {
            from_subaccount: None,
            to: Account {
                owner: to,
                subaccount: None,
            },
            fee: None,
            created_at_time: None,
            memo: None,
            amount: amount.to_nat(),
        })
        .await
        .map_err(|e| TransferError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64), 
            message: e.1,
        })??;
    
    Ok(block_index.0.to_u64().unwrap())
}

pub async fn transfer_icusd_from(amount: ICUSD, caller: Principal) -> Result<u64, TransferFromError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: read_state(|s| s.icusd_ledger_principal),
    };
    let protocol_id = ic_cdk::id();
    let block_index = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: None,
            from: Account {
                owner: caller,
                subaccount: None,
            },
            to: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: amount.to_nat(),
            fee: None,
            created_at_time: None,
            memo: None,
        })
        .await
        .map_err(|e| TransferFromError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64), 
            message: e.1,                            
        })?;
        
    
        Ok(block_index.unwrap().0.to_u64().unwrap())
}


/// Thin wrapper around generic transfer_collateral_from for ICP.
pub async fn transfer_icp_from(amount: ICP, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral_from(amount.to_u64(), caller, ledger).await
}

/// Thin wrapper around generic transfer_collateral for ICP.
pub async fn transfer_icp(amount: ICP, to: Principal) -> Result<u64, TransferError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral(amount.to_u64(), to, ledger).await
}

pub async fn transfer_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: read_state(|s| s.icusd_ledger_principal),
    };
    let block_index = client
        .transfer(TransferArg {
            from_subaccount: None,
            to: Account {
                owner: to,
                subaccount: None,
            },
            fee: None,
            created_at_time: None,
            memo: None,
            amount: amount.to_nat(),
        })
        .await
        .map_err(|e| TransferError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64),
            message: e.1,
        })??;

    Ok(block_index.0.to_u64().unwrap())
}

/// Generic collateral transfer: move tokens from the protocol canister to a recipient.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
pub async fn transfer_collateral(amount: u64, to: Principal, ledger: Principal) -> Result<u64, TransferError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let block_index = client
        .transfer(TransferArg {
            from_subaccount: None,
            to: Account {
                owner: to,
                subaccount: None,
            },
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(amount),
        })
        .await
        .map_err(|e| TransferError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64),
            message: e.1,
        })??;

    Ok(block_index.0.to_u64().unwrap())
}

/// Generic collateral transfer_from: pull tokens from a user into the protocol canister.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
pub async fn transfer_collateral_from(amount: u64, from: Principal, ledger: Principal) -> Result<u64, TransferFromError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let protocol_id = ic_cdk::id();
    let block_index = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: None,
            from: Account {
                owner: from,
                subaccount: None,
            },
            to: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: Nat::from(amount),
            fee: None,
            created_at_time: None,
            memo: None,
        })
        .await
        .map_err(|e| TransferFromError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64),
            message: e.1,
        })?;

    Ok(block_index.unwrap().0.to_u64().unwrap())
}

/// Transfer ckUSDT or ckUSDC from a user to the protocol (for vault repayment/liquidation)
/// Amount is in e6s (6-decimal stable token units)
pub async fn transfer_stable_from(token_type: StableTokenType, amount_e6s: u64, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger_principal = match token_type {
        StableTokenType::CKUSDT => read_state(|s| s.ckusdt_ledger_principal),
        StableTokenType::CKUSDC => read_state(|s| s.ckusdc_ledger_principal),
    }.ok_or_else(|| TransferFromError::GenericError {
        error_code: Nat::from(0u64),
        message: format!("{:?} ledger not configured", token_type),
    })?;

    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger_principal,
    };
    let protocol_id = ic_cdk::id();
    let block_index = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: None,
            from: Account {
                owner: caller,
                subaccount: None,
            },
            to: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: Nat::from(amount_e6s),
            fee: None,
            created_at_time: None,
            memo: None,
        })
        .await
        .map_err(|e| TransferFromError::GenericError {
            error_code: Nat::from(e.0.max(0) as u64),
            message: e.1,
        })?;

    Ok(block_index.unwrap().0.to_u64().unwrap())
}



