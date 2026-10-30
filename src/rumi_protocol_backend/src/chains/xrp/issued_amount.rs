//! Exact XRPL issued-currency amounts and token identity (pure, no I/O).
//!
//! XRPL issued amounts are a sign, a 16-digit decimal mantissa in
//! `[1e15, 1e16)` and a base-10 exponent in `[-96, 80]` (or the single canonical
//! zero). Money never touches `f64` or a lossy `Decimal` here: every operation is
//! integer arithmetic on the canonical form and fails (never rounds) when the
//! exact result is not representable. Receipt parsing must keep arbitrary valid
//! mantissa/exponent pairs (sub-e8s dust included) so a held claim stays an exact
//! owed amount; the stricter "normal request" validator is separate.
//!
//! The 64-bit wire encoding and the JSON decimal rendering follow rippled's
//! `STAmount` (`getText`) so values round-trip identically with the XRPL SDKs.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::address::{account_id_from_classic_address, classic_address_from_account_id};

/// Smallest/largest canonical mantissa (16 digits).
pub const MIN_MANTISSA: u64 = 1_000_000_000_000_000;
pub const MAX_MANTISSA: u64 = 9_999_999_999_999_999;
pub const MIN_EXPONENT: i32 = -96;
pub const MAX_EXPONENT: i32 = 80;
/// icUSD uses 8 decimals everywhere (1 e8s == 1e-8 icUSD).
pub const E8S_DECIMALS: u32 = 8;
/// Normal request bounds: <= 8 decimals and <= 15 significant digits.
pub const NORMAL_MAX_SIGNIFICANT_DIGITS: u32 = 15;
pub const NORMAL_MAX_DECIMALS: u32 = 8;
/// Longest accepted textual amount (bounds hostile input before any parsing).
pub const MAX_AMOUNT_TEXT_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AmountError {
    Empty,
    TooLong,
    Malformed(String),
    /// More than 16 significant digits: cannot be an exact XRPL amount.
    TooPrecise,
    /// Exponent outside `[-96, 80]` once normalised (over/underflow).
    OutOfRange,
    Negative,
    /// Not an integral number of e8s.
    NotExactE8s,
    /// Does not fit the target integer type.
    Overflow,
    /// Exact sum/difference would need more than 16 significant digits.
    Inexact,
    NotPositive,
    TooManyDecimals,
    TooManySignificantDigits,
    InvalidWire(String),
}

impl fmt::Display for AmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AmountError {}

fn pow10_u128(n: u32) -> u128 {
    10u128.pow(n)
}

/// A validated canonical XRPL issued amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ExactIssuedAmount {
    negative: bool,
    mantissa: u64,
    exponent: i32,
}

#[derive(Deserialize)]
struct ExactIssuedAmountRaw {
    negative: bool,
    mantissa: u64,
    exponent: i32,
}

impl<'de> Deserialize<'de> for ExactIssuedAmount {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = ExactIssuedAmountRaw::deserialize(d)?;
        ExactIssuedAmount::from_canonical_parts(raw.negative, raw.mantissa, raw.exponent)
            .map_err(serde::de::Error::custom)
    }
}

impl ExactIssuedAmount {
    pub const ZERO: ExactIssuedAmount = ExactIssuedAmount {
        negative: false,
        mantissa: 0,
        exponent: 0,
    };

    /// Build from already-canonical parts, rejecting anything non-canonical.
    pub fn from_canonical_parts(
        negative: bool,
        mantissa: u64,
        exponent: i32,
    ) -> Result<Self, AmountError> {
        if mantissa == 0 {
            if negative || exponent != 0 {
                return Err(AmountError::Malformed("non-canonical zero".into()));
            }
            return Ok(Self::ZERO);
        }
        if !(MIN_MANTISSA..=MAX_MANTISSA).contains(&mantissa)
            || !(MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent)
        {
            return Err(AmountError::OutOfRange);
        }
        Ok(Self {
            negative,
            mantissa,
            exponent,
        })
    }

    /// Normalise `±mantissa × 10^exp10` into canonical form, failing (never
    /// rounding) if that needs more than 16 significant digits or leaves range.
    pub fn from_parts(negative: bool, mantissa: u128, exp10: i64) -> Result<Self, AmountError> {
        if mantissa == 0 {
            return Ok(Self::ZERO);
        }
        let mut m = mantissa;
        let mut e = exp10;
        while m % 10 == 0 {
            m /= 10;
            e += 1;
        }
        if m > MAX_MANTISSA as u128 {
            return Err(AmountError::TooPrecise);
        }
        while m < MIN_MANTISSA as u128 {
            m *= 10;
            e -= 1;
        }
        if e < MIN_EXPONENT as i64 || e > MAX_EXPONENT as i64 {
            return Err(AmountError::OutOfRange);
        }
        Ok(Self {
            negative,
            mantissa: m as u64,
            exponent: e as i32,
        })
    }

    /// Parse a decimal or scientific string exactly (`"12.5"`, `"-1e-7"`,
    /// `"9999999999999999e80"`). Excess precision is an error, not a rounding.
    pub fn parse(s: &str) -> Result<Self, AmountError> {
        if s.is_empty() {
            return Err(AmountError::Empty);
        }
        if s.len() > MAX_AMOUNT_TEXT_LEN {
            return Err(AmountError::TooLong);
        }
        let bytes = s.as_bytes();
        let mut i = 0usize;
        let mut negative = false;
        if bytes[i] == b'-' || bytes[i] == b'+' {
            negative = bytes[i] == b'-';
            i += 1;
        }
        let mut digits: Vec<u8> = Vec::new();
        let mut frac_digits: i64 = 0;
        let mut seen_point = false;
        let mut seen_digit = false;
        while i < bytes.len() {
            let c = bytes[i];
            match c {
                b'0'..=b'9' => {
                    digits.push(c - b'0');
                    seen_digit = true;
                    if seen_point {
                        frac_digits += 1;
                    }
                }
                b'.' if !seen_point => seen_point = true,
                _ => break,
            }
            i += 1;
        }
        if !seen_digit {
            return Err(AmountError::Malformed("no digits".into()));
        }
        let mut exp: i64 = 0;
        if i < bytes.len() {
            if bytes[i] != b'e' && bytes[i] != b'E' {
                return Err(AmountError::Malformed("unexpected character".into()));
            }
            i += 1;
            let mut exp_neg = false;
            if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
                exp_neg = bytes[i] == b'-';
                i += 1;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                exp = exp
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((bytes[i] - b'0') as i64))
                    .filter(|v| *v < 1_000_000)
                    .ok_or(AmountError::OutOfRange)?;
                i += 1;
            }
            if i == start || i != bytes.len() {
                return Err(AmountError::Malformed("bad exponent".into()));
            }
            if exp_neg {
                exp = -exp;
            }
        }
        // Strip leading zeros, then trailing zeros (adjusting the exponent).
        let first = digits.iter().position(|d| *d != 0);
        let Some(first) = first else {
            return Ok(Self::ZERO);
        };
        let mut exp10 = exp - frac_digits;
        let mut end = digits.len();
        while end > first && digits[end - 1] == 0 {
            end -= 1;
            exp10 += 1;
        }
        let sig = &digits[first..end];
        if sig.len() > 16 {
            return Err(AmountError::TooPrecise);
        }
        let mut m: u128 = 0;
        for d in sig {
            m = m * 10 + *d as u128;
        }
        Self::from_parts(negative, m, exp10)
    }

    /// Parse and apply the normal-request validator (positive, <= 8 decimals,
    /// <= 15 significant digits).
    pub fn parse_normal_request(s: &str) -> Result<Self, AmountError> {
        let a = Self::parse(s)?;
        a.validate_normal_request()?;
        Ok(a)
    }

    /// Normal user-request rule: strictly positive, at most 8 decimal places and 15
    /// significant digits, and representable in u64 e8s. Returns the e8s value.
    pub fn validate_normal_request(&self) -> Result<u64, AmountError> {
        if self.negative {
            return Err(AmountError::Negative);
        }
        if self.is_zero() {
            return Err(AmountError::NotPositive);
        }
        let (sig, decimals) = self.sig_digits_and_decimals();
        if decimals > NORMAL_MAX_DECIMALS {
            return Err(AmountError::TooManyDecimals);
        }
        if sig > NORMAL_MAX_SIGNIFICANT_DIGITS {
            return Err(AmountError::TooManySignificantDigits);
        }
        self.to_e8s_exact()
    }

    fn sig_digits_and_decimals(&self) -> (u32, u32) {
        let mut m = self.mantissa;
        let mut e = self.exponent as i64;
        let mut digits = 16u32;
        while m % 10 == 0 {
            m /= 10;
            e += 1;
            digits -= 1;
        }
        let decimals = if e < 0 { (-e) as u32 } else { 0 };
        (digits, decimals)
    }

    pub fn is_zero(&self) -> bool {
        self.mantissa == 0
    }
    pub fn is_negative(&self) -> bool {
        self.negative
    }
    pub fn mantissa(&self) -> u64 {
        self.mantissa
    }
    pub fn exponent(&self) -> i32 {
        self.exponent
    }

    pub fn from_e8s(e8s: u64) -> Self {
        // u64 -> at most 20 digits; from_parts only fails past 16 significant
        // digits, so route the (rare) wide case through exact splitting.
        match Self::from_parts(false, e8s as u128, -(E8S_DECIMALS as i64)) {
            Ok(v) => v,
            // > 16 significant digits only for e8s >= 1e16 with a non-zero tail:
            // not representable exactly as an XRPL amount. Saturate is forbidden,
            // so callers must use `try_from_e8s` for untrusted magnitudes.
            Err(_) => panic!("from_e8s: {e8s} needs more than 16 significant digits"),
        }
    }

    /// Fallible variant of [`Self::from_e8s`].
    pub fn try_from_e8s(e8s: u64) -> Result<Self, AmountError> {
        Self::from_parts(false, e8s as u128, -(E8S_DECIMALS as i64))
    }

    /// Exact conversion to e8s; errors unless the value is a non-negative whole
    /// number of e8s that fits `u64`.
    pub fn to_e8s_exact(&self) -> Result<u64, AmountError> {
        if self.negative {
            return Err(AmountError::Negative);
        }
        let (floor, rem) = self.split_e8s()?;
        if !rem.is_zero() {
            return Err(AmountError::NotExactE8s);
        }
        Ok(floor)
    }

    /// Split a non-negative amount into `(floor e8s, exact sub-e8s remainder)`.
    pub fn split_e8s(&self) -> Result<(u64, ExactIssuedAmount), AmountError> {
        if self.negative {
            return Err(AmountError::Negative);
        }
        if self.is_zero() {
            return Ok((0, Self::ZERO));
        }
        let shift = self.exponent as i64 + E8S_DECIMALS as i64;
        if shift >= 0 {
            let scaled = (self.mantissa as u128)
                .checked_mul(pow10_u128(shift.min(38) as u32))
                .filter(|_| shift <= 38)
                .ok_or(AmountError::Overflow)?;
            let e8s = u64::try_from(scaled).map_err(|_| AmountError::Overflow)?;
            return Ok((e8s, Self::ZERO));
        }
        let k = (-shift) as u32;
        if k > 16 {
            // mantissa < 1e16 <= 10^k, so the whole value is a sub-e8s remainder.
            return Ok((0, *self));
        }
        let div = pow10_u128(k);
        let m = self.mantissa as u128;
        let floor = m / div;
        let rem = m % div;
        let rem_amt = Self::from_parts(false, rem, self.exponent as i64)?;
        Ok((
            u64::try_from(floor).map_err(|_| AmountError::Overflow)?,
            rem_amt,
        ))
    }

    pub fn negated(&self) -> Self {
        if self.is_zero() {
            *self
        } else {
            Self {
                negative: !self.negative,
                ..*self
            }
        }
    }

    fn signed_mantissa(&self) -> i128 {
        if self.negative {
            -(self.mantissa as i128)
        } else {
            self.mantissa as i128
        }
    }

    /// Exact addition; errors with `Inexact` rather than rounding.
    pub fn checked_add(&self, other: &Self) -> Result<Self, AmountError> {
        if self.is_zero() {
            return Ok(*other);
        }
        if other.is_zero() {
            return Ok(*self);
        }
        let (hi, lo) = if self.exponent >= other.exponent {
            (self, other)
        } else {
            (other, self)
        };
        let d = (hi.exponent - lo.exponent) as u32;
        // With both mantissas 16 digits wide, a gap beyond 16 decades always yields
        // an unrepresentable (>16 digit) exact sum.
        if d > 16 {
            return Err(AmountError::Inexact);
        }
        let hi_v = hi.signed_mantissa() * pow10_u128(d) as i128;
        let sum = hi_v + lo.signed_mantissa();
        Self::from_parts(sum < 0, sum.unsigned_abs(), lo.exponent as i64).map_err(|e| {
            if e == AmountError::TooPrecise {
                AmountError::Inexact
            } else {
                e
            }
        })
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, AmountError> {
        self.checked_add(&other.negated())
    }

    /// 64-bit XRPL binary amount (not-XRP bit, sign bit, exponent+97, mantissa).
    pub fn to_wire_u64(&self) -> u64 {
        if self.is_zero() {
            return 0x8000_0000_0000_0000;
        }
        let sign = if self.negative {
            0
        } else {
            0x4000_0000_0000_0000u64
        };
        0x8000_0000_0000_0000u64 | sign | (((self.exponent + 97) as u64) << 54) | self.mantissa
    }

    pub fn from_wire_u64(w: u64) -> Result<Self, AmountError> {
        if w & 0x8000_0000_0000_0000 == 0 {
            return Err(AmountError::InvalidWire("native XRP amount".into()));
        }
        if w == 0x8000_0000_0000_0000 {
            return Ok(Self::ZERO);
        }
        let positive = w & 0x4000_0000_0000_0000 != 0;
        let exp_raw = ((w >> 54) & 0xFF) as i32;
        let mantissa = w & 0x003F_FFFF_FFFF_FFFF;
        Self::from_canonical_parts(!positive, mantissa, exp_raw - 97)
            .map_err(|e| AmountError::InvalidWire(format!("{e}")))
    }

    /// rippled's canonical JSON text (`STAmount::getText`): plain decimal for
    /// exponents in `[-25, -5]` or `0`, otherwise `<mantissa>e<exponent>`.
    pub fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        let sign = if self.negative { "-" } else { "" };
        if self.exponent != 0 && !(-25..=-5).contains(&self.exponent) {
            return format!("{sign}{}e{}", self.mantissa, self.exponent);
        }
        let digits = self.mantissa.to_string(); // always 16 digits
        let pos = 16 + self.exponent as i64; // digits before the decimal point
        let body = if pos <= 0 {
            let frac = format!("{}{}", "0".repeat((-pos) as usize), digits);
            format!("0.{}", frac.trim_end_matches('0'))
        } else if pos >= 16 {
            format!("{digits}{}", "0".repeat((pos - 16) as usize))
        } else {
            let (int, frac) = digits.split_at(pos as usize);
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                int.to_string()
            } else {
                format!("{int}.{frac}")
            }
        };
        format!("{sign}{body}")
    }
}

impl PartialOrd for ExactIssuedAmount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExactIssuedAmount {
    fn cmp(&self, other: &Self) -> Ordering {
        fn rank(a: &ExactIssuedAmount) -> i8 {
            if a.is_zero() {
                0
            } else if a.negative {
                -1
            } else {
                1
            }
        }
        let (ra, rb) = (rank(self), rank(other));
        if ra != rb {
            return ra.cmp(&rb);
        }
        let mag = (self.exponent, self.mantissa).cmp(&(other.exponent, other.mantissa));
        if ra < 0 {
            mag.reverse()
        } else {
            mag
        }
    }
}

impl fmt::Display for ExactIssuedAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_decimal_string())
    }
}

// ---------------------------------------------------------------------------
// Token identity
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    InvalidCurrency(String),
    InvalidIssuer(String),
    /// Same symbol, wrong issuer (or wrong currency): never credit.
    WrongAsset {
        expected: String,
        got: String,
    },
    BadAmountJson(String),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// icUSD 160-bit currency code: ASCII "icUSD" right-padded with zeros.
pub const ICUSD_CURRENCY_HEX: &str = "6963555344000000000000000000000000000000";

/// A 160-bit XRPL currency code. Non-standard (hex) codes must not start with
/// 0x00 (that prefix is reserved for the 3-character standard form), which also
/// removes the dual-representation ambiguity between "USD" and its padded hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CurrencyCode(pub [u8; 20]);

const STANDARD_CODE_CHARS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789?!@#$%^&*<>(){}[]|";

impl CurrencyCode {
    pub fn icusd() -> Self {
        Self::from_hex(ICUSD_CURRENCY_HEX).expect("icUSD currency constant is valid")
    }

    pub fn from_bytes(b: [u8; 20]) -> Result<Self, AssetError> {
        if b[0] != 0 {
            return Ok(Self(b));
        }
        // 0x00 prefix: only the standard 3-character layout is allowed.
        let layout_ok = b[..12].iter().all(|x| *x == 0) && b[15..].iter().all(|x| *x == 0);
        if !layout_ok {
            return Err(AssetError::InvalidCurrency(
                "0x00-prefixed code must be the standard 3-character layout".into(),
            ));
        }
        let code = &b[12..15];
        if code
            .iter()
            .any(|c| !STANDARD_CODE_CHARS.as_bytes().contains(c))
        {
            return Err(AssetError::InvalidCurrency(
                "illegal standard code char".into(),
            ));
        }
        if code == b"XRP" {
            return Err(AssetError::InvalidCurrency("XRP is reserved".into()));
        }
        Ok(Self(b))
    }

    /// Accepts a 3-character standard code or a 40-hex non-standard code whose
    /// first byte is non-zero.
    pub fn parse(s: &str) -> Result<Self, AssetError> {
        if s.len() == 3 {
            let mut b = [0u8; 20];
            b[12..15].copy_from_slice(s.as_bytes());
            return Self::from_bytes(b);
        }
        if s.len() == 40 {
            let c = Self::from_hex(s)?;
            if c.0[0] == 0 {
                return Err(AssetError::InvalidCurrency(
                    "hex form of a standard code is ambiguous; use the 3-character form".into(),
                ));
            }
            return Ok(c);
        }
        Err(AssetError::InvalidCurrency(format!(
            "bad length {}",
            s.len()
        )))
    }

    pub fn from_hex(s: &str) -> Result<Self, AssetError> {
        let v = hex::decode(s).map_err(|e| AssetError::InvalidCurrency(e.to_string()))?;
        let b: [u8; 20] = v
            .try_into()
            .map_err(|_| AssetError::InvalidCurrency("expected 20 bytes".into()))?;
        Self::from_bytes(b)
    }

    pub fn to_hex(&self) -> String {
        hex::encode_upper(self.0)
    }

    /// rippled renders standard codes as 3 characters and others as 40 hex.
    pub fn to_rpc_string(&self) -> String {
        if self.0[0] == 0 {
            String::from_utf8_lossy(&self.0[12..15]).into_owned()
        } else {
            self.to_hex()
        }
    }

    /// Compare against a currency string from an RPC response.
    pub fn matches_rpc(&self, s: &str) -> bool {
        Self::parse(s).map(|c| c == *self).unwrap_or(false)
    }
}

/// Issued asset identity: currency + issuer account (network is carried
/// separately by the caller / config).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssuedAsset {
    pub currency: CurrencyCode,
    pub issuer: [u8; 20],
}

impl IssuedAsset {
    pub fn new(currency: CurrencyCode, issuer: [u8; 20]) -> Self {
        Self { currency, issuer }
    }

    pub fn from_classic(currency: CurrencyCode, issuer_classic: &str) -> Result<Self, AssetError> {
        let issuer =
            account_id_from_classic_address(issuer_classic).map_err(AssetError::InvalidIssuer)?;
        Ok(Self { currency, issuer })
    }

    pub fn icusd(issuer_classic: &str) -> Result<Self, AssetError> {
        Self::from_classic(CurrencyCode::icusd(), issuer_classic)
    }

    pub fn issuer_address(&self) -> String {
        classic_address_from_account_id(&self.issuer)
    }

    /// Validate an RPC `{currency, issuer, value}` amount object against this
    /// asset (currency AND issuer) and return the exact value.
    pub fn check_rpc_amount(&self, v: &Value) -> Result<ExactIssuedAmount, AssetError> {
        let obj = v
            .as_object()
            .ok_or_else(|| AssetError::BadAmountJson("not an issued-currency object".into()))?;
        let cur = obj
            .get("currency")
            .and_then(Value::as_str)
            .ok_or_else(|| AssetError::BadAmountJson("missing currency".into()))?;
        let iss = obj
            .get("issuer")
            .and_then(Value::as_str)
            .ok_or_else(|| AssetError::BadAmountJson("missing issuer".into()))?;
        let value = obj
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| AssetError::BadAmountJson("missing value".into()))?;
        self.check_identity(cur, iss)?;
        ExactIssuedAmount::parse(value).map_err(|e| AssetError::BadAmountJson(e.to_string()))
    }

    /// Currency and issuer must both match; a same-symbol token from another
    /// issuer is rejected.
    pub fn check_identity(&self, currency: &str, issuer: &str) -> Result<(), AssetError> {
        let issuer_id =
            account_id_from_classic_address(issuer).map_err(AssetError::InvalidIssuer)?;
        if !self.currency.matches_rpc(currency) || issuer_id != self.issuer {
            return Err(AssetError::WrongAsset {
                expected: format!(
                    "{}:{}",
                    self.currency.to_rpc_string(),
                    self.issuer_address()
                ),
                got: format!("{currency}:{issuer}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ExactIssuedAmount {
        ExactIssuedAmount::parse(s).unwrap()
    }

    #[test]
    fn amount_scientific_and_fractional_exact() {
        // Same value, different spellings, one canonical form.
        assert_eq!(p("1.5"), p("15e-1"));
        assert_eq!(p("1.5"), p("0.15E1"));
        assert_eq!(p("1.5"), p("1500e-3"));
        assert_eq!(p("100"), p("1e2"));
        assert_eq!(p("0.000"), ExactIssuedAmount::ZERO);
        assert_eq!(p("-0"), ExactIssuedAmount::ZERO);
        assert_eq!(p("1.5e-7").to_wire_u64(), p("0.00000015").to_wire_u64());
        let a = p("0.0000000012345678");
        assert_eq!(a.mantissa(), 1_234_567_800_000_000);
        assert_eq!(a.exponent(), -24);
        // f64-hostile value stays exact.
        assert_eq!(p("0.1").checked_add(&p("0.2")).unwrap(), p("0.3"));
        // Excess precision is an error, never a rounding.
        assert_eq!(
            ExactIssuedAmount::parse("1.00000000000000001"),
            Err(AmountError::TooPrecise)
        );
        assert_eq!(
            ExactIssuedAmount::parse("1e96"),
            Err(AmountError::OutOfRange)
        );
        assert!(ExactIssuedAmount::parse("1e95").is_ok());
        assert!(ExactIssuedAmount::parse("1e-81").is_ok());
        assert_eq!(
            ExactIssuedAmount::parse("1e-82"),
            Err(AmountError::OutOfRange)
        );
        assert_eq!(
            ExactIssuedAmount::parse("1e-100"),
            Err(AmountError::OutOfRange)
        );
        for bad in [
            "",
            "-",
            ".",
            "e5",
            "1e",
            "1e+",
            "1.2.3",
            "1x",
            " 1",
            "1 ",
            "0x10",
            "1e99999999999",
        ] {
            assert!(
                ExactIssuedAmount::parse(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert_eq!(
            ExactIssuedAmount::parse(&"1".repeat(200)),
            Err(AmountError::TooLong)
        );
        // Range extremes round-trip through the wire form.
        for s in [
            "9999999999999999e80",
            "1000000000000000e-96",
            "1",
            "-123.456",
        ] {
            let a = p(s);
            assert_eq!(
                ExactIssuedAmount::from_wire_u64(a.to_wire_u64()).unwrap(),
                a,
                "{s}"
            );
        }
    }

    #[test]
    fn e8s_conversions_are_exact() {
        assert_eq!(p("1").to_e8s_exact(), Ok(100_000_000));
        assert_eq!(p("0.00000001").to_e8s_exact(), Ok(1));
        assert_eq!(
            p("9999999.99999999").to_e8s_exact(),
            Ok(999_999_999_999_999)
        );
        assert_eq!(
            p("0.000000001").to_e8s_exact(),
            Err(AmountError::NotExactE8s)
        );
        assert_eq!(p("-1").to_e8s_exact(), Err(AmountError::Negative));
        assert_eq!(p("1e30").to_e8s_exact(), Err(AmountError::Overflow));
        assert_eq!(p("1e80").to_e8s_exact(), Err(AmountError::Overflow));
        assert_eq!(p("0").to_e8s_exact(), Ok(0));

        let (floor, rem) = p("1.234567891234567").split_e8s().unwrap();
        assert_eq!(floor, 123_456_789);
        assert_eq!(rem, p("0.000000001234567"));
        assert_eq!(
            rem.checked_add(&ExactIssuedAmount::from_e8s(floor))
                .unwrap(),
            p("1.234567891234567")
        );
        // Entirely sub-e8s.
        let (floor, rem) = p("0.0000000012345678").split_e8s().unwrap();
        assert_eq!((floor, rem), (0, p("0.0000000012345678")));
        let (floor, rem) = p("1e-40").split_e8s().unwrap();
        assert_eq!((floor, rem), (0, p("1e-40")));
        assert_eq!(ExactIssuedAmount::from_e8s(123_456_789), p("1.23456789"));
        assert_eq!(ExactIssuedAmount::from_e8s(0), ExactIssuedAmount::ZERO);
        assert!(ExactIssuedAmount::try_from_e8s(u64::MAX).is_err());
    }

    #[test]
    fn ordering_and_arithmetic_are_exact() {
        assert!(p("-2") < p("-1"));
        assert!(p("-1") < ExactIssuedAmount::ZERO);
        assert!(ExactIssuedAmount::ZERO < p("1e-81"));
        assert!(p("1") < p("1.000000000000001"));
        assert!(p("9") < p("10"));
        assert!(p("1e-81") < p("1e-80"));
        assert_eq!(
            p("5").checked_sub(&p("5")).unwrap(),
            ExactIssuedAmount::ZERO
        );
        assert_eq!(p("1").checked_sub(&p("3")).unwrap(), p("-2"));
        assert_eq!(
            p("100").checked_add(&p("0.00000001")).unwrap(),
            p("100.00000001")
        );
        assert_eq!(p("1e20").checked_add(&p("1")), Err(AmountError::Inexact));
        assert_eq!(p("1e80").checked_add(&p("1e80")).unwrap(), p("2e80"));
        assert_eq!(
            p("9999999999999999e80").checked_add(&p("9999999999999999e80")),
            Err(AmountError::Inexact)
        );
        let mut v = vec![p("3"), p("-1"), p("0"), p("2.5"), p("1e-8")];
        v.sort();
        assert_eq!(v, vec![p("-1"), p("0"), p("1e-8"), p("2.5"), p("3")]);
    }

    #[test]
    fn normal_request_validator_bounds() {
        let ok = ExactIssuedAmount::parse_normal_request("9999999.99999999").unwrap();
        assert_eq!(ok.validate_normal_request(), Ok(999_999_999_999_999));
        assert!(ExactIssuedAmount::parse_normal_request("12.345678").is_ok());
        assert_eq!(
            ExactIssuedAmount::parse_normal_request("0.000000001").unwrap_err(),
            AmountError::TooManyDecimals
        );
        // 16 significant digits with <= 8 decimals.
        assert_eq!(
            ExactIssuedAmount::parse_normal_request("99999999.99999999").unwrap_err(),
            AmountError::TooManySignificantDigits
        );
        assert_eq!(
            ExactIssuedAmount::parse_normal_request("0").unwrap_err(),
            AmountError::NotPositive
        );
        assert_eq!(
            ExactIssuedAmount::parse_normal_request("-1").unwrap_err(),
            AmountError::Negative
        );
        // 15 digits but beyond u64 e8s.
        assert_eq!(
            ExactIssuedAmount::parse_normal_request("100000000000000").unwrap_err(),
            AmountError::Overflow
        );
        assert!(ExactIssuedAmount::parse_normal_request("1e-8").is_ok());
    }

    #[test]
    fn decimal_rendering_matches_rippled() {
        for (input, text) in [
            ("100.5", "100.5"),
            ("25.12345678", "25.12345678"),
            ("9999999.99999999", "9999999.99999999"),
            ("0.0000000012345678", "0.0000000012345678"),
            ("1.5e-7", "0.00000015"),
            ("1000000", "1000000"),
            ("0.5", "0.5"),
            ("-3.25", "-3.25"),
            ("0", "0"),
            ("1e15", "1000000000000000"),
            ("1e-5", "0.00001"),
            // exponent > -5 (but != 0) or < -25 switches to <mantissa>e<exp>.
            ("1e-4", "0.0001"),
            ("1e10", "10000000000"),
            ("1e11", "1000000000000000e-4"),
            ("1e12", "1000000000000000e-3"),
            ("1e-30", "1000000000000000e-45"),
            ("1e-10", "0.0000000001"),
            ("1e-11", "1000000000000000e-26"),
            ("9999999999999999e80", "9999999999999999e80"),
        ] {
            let a = p(input);
            assert_eq!(a.to_decimal_string(), text, "{input}");
            assert_eq!(
                ExactIssuedAmount::parse(&a.to_decimal_string()).unwrap(),
                a,
                "{input} reparse"
            );
        }
    }

    #[test]
    fn wire_encoding_known_values() {
        // 1 icUSD = mantissa 1e15, exp -15 -> (-15+97)=82.
        assert_eq!(p("1").to_wire_u64(), 0xD483_8D7E_A4C6_8000);
        assert_eq!(ExactIssuedAmount::ZERO.to_wire_u64(), 0x8000_0000_0000_0000);
        assert_eq!(p("-1").to_wire_u64(), 0x9483_8D7E_A4C6_8000);
        assert!(ExactIssuedAmount::from_wire_u64(0x4000_0000_0000_0001).is_err());
        assert!(ExactIssuedAmount::from_wire_u64(0xC000_0000_0000_0001).is_err()); // non-canonical mantissa
        assert!(ExactIssuedAmount::from_wire_u64(0x8000_0000_0000_0001).is_err());
    }

    #[test]
    fn serde_rejects_non_canonical() {
        let ok = serde_json::to_string(&p("1.5")).unwrap();
        let back: ExactIssuedAmount = serde_json::from_str(&ok).unwrap();
        assert_eq!(back, p("1.5"));
        assert!(serde_json::from_str::<ExactIssuedAmount>(
            r#"{"negative":false,"mantissa":5,"exponent":0}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ExactIssuedAmount>(
            r#"{"negative":true,"mantissa":0,"exponent":0}"#
        )
        .is_err());
    }

    const ISSUER_A: &str = "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD";
    const ISSUER_B: &str = "rUn84CUYbNjRoTQ6mSW7BVJPSVJNLb1QLo";

    #[test]
    fn currency_code_rules() {
        assert_eq!(
            CurrencyCode::icusd().to_hex(),
            ICUSD_CURRENCY_HEX.to_uppercase()
        );
        assert_eq!(&CurrencyCode::icusd().0[..5], b"icUSD");
        assert!(CurrencyCode::parse("USD").is_ok());
        assert!(CurrencyCode::parse("XRP").is_err());
        assert!(CurrencyCode::parse("US ").is_err());
        // Hex form of a standard code is the ambiguous representation: rejected.
        assert!(CurrencyCode::parse("0000000000000000000000005553440000000000").is_err());
        assert!(CurrencyCode::parse("0000000000000000000000000000000000000000").is_err());
        assert!(CurrencyCode::parse("0100000000000000000000000000000000000000").is_ok());
        assert!(CurrencyCode::parse("6963555344").is_err());
        assert!(CurrencyCode::icusd().matches_rpc("6963555344000000000000000000000000000000"));
        assert!(CurrencyCode::icusd().matches_rpc(
            "6963555344000000000000000000000000000000"
                .to_lowercase()
                .as_str()
        ));
        assert!(!CurrencyCode::icusd().matches_rpc("USD"));
        assert_eq!(CurrencyCode::parse("USD").unwrap().to_rpc_string(), "USD");
    }

    #[test]
    fn wrong_issuer_same_symbol_rejected() {
        let asset = IssuedAsset::icusd(ISSUER_A).unwrap();
        let good = serde_json::json!({"currency": ICUSD_CURRENCY_HEX, "issuer": ISSUER_A, "value": "12.5"});
        assert_eq!(asset.check_rpc_amount(&good).unwrap(), p("12.5"));
        let evil = serde_json::json!({"currency": ICUSD_CURRENCY_HEX, "issuer": ISSUER_B, "value": "12.5"});
        assert!(matches!(
            asset.check_rpc_amount(&evil),
            Err(AssetError::WrongAsset { .. })
        ));
        let wrong_cur = serde_json::json!({"currency": "USD", "issuer": ISSUER_A, "value": "12.5"});
        assert!(matches!(
            asset.check_rpc_amount(&wrong_cur),
            Err(AssetError::WrongAsset { .. })
        ));
        let native = serde_json::json!("12500000");
        assert!(matches!(
            asset.check_rpc_amount(&native),
            Err(AssetError::BadAmountJson(_))
        ));
        let bad_issuer = serde_json::json!({"currency": ICUSD_CURRENCY_HEX, "issuer": "rNotAnAddress", "value": "1"});
        assert!(matches!(
            asset.check_rpc_amount(&bad_issuer),
            Err(AssetError::InvalidIssuer(_))
        ));
        assert_eq!(asset.issuer_address(), ISSUER_A);
    }
}
