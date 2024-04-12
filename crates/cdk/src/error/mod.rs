use std::string::FromUtf8Error;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::util::hex;

pub mod mint;
pub mod wallet;

#[derive(Debug, Error)]
pub enum Error {
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    #[error("No Key for Amoun")]
    AmountKey,
    #[error("Amount miss match")]
    Amount,
    #[error("Token already spent")]
    TokenSpent,
    #[error("Token not verified")]
    TokenNotVerifed,
    #[error("Invoice Amount undefined")]
    InvoiceAmountUndefined,
    #[error("Proof missing required field")]
    MissingProofField,
    #[error("No valid point found")]
    NoValidPoint,
    #[error("Kind not found")]
    KindNotFound,
    #[error("Unknown Tag")]
    UnknownTag,
    #[error("Incorrect Secret Kind")]
    IncorrectSecretKind,
    #[error("Spending conditions not met")]
    SpendConditionsNotMet,
    #[error("Could not convert key")]
    Key,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Locktime in past")]
    LocktimeInPast,
    #[error(transparent)]
    Secret(#[from] super::secret::Error),
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    #[cfg(feature = "nut13")]
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Custom error
    #[error("`{0}`")]
    CustomError(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: u32,
    pub error: Option<String>,
    pub detail: Option<String>,
}

impl ErrorResponse {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        if let Ok(res) = serde_json::from_str::<ErrorResponse>(json) {
            Ok(res)
        } else {
            Ok(Self {
                code: 999,
                error: Some(json.to_string()),
                detail: None,
            })
        }
    }
}