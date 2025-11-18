use std::collections::HashMap;
use tokio::time::{timeout, Duration};

use cdk_common::nut04::MintMethodOptions;
use cdk_common::wallet::{Transaction, TransactionDirection, TransactionKind};
use tracing::instrument;

use super::MintQuote;
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut12, MintQuoteBolt11Request, MintQuoteBolt11Response, MintRequest, PreMintSecrets, Proofs,
    SecretKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::util::unix_time;
use crate::wallet::MintQuoteState;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use cdk::amount::Amount;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 32]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<(MintQuote, Transaction), Error> {
        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let settings = self
                .localstore
                .get_mint(mint_url.clone())
                .await?
                .ok_or(Error::IncorrectMint)?
                .nuts
                .nut04
                .get_settings(&unit, &crate::nuts::PaymentMethod::Bolt11)
                .ok_or(Error::UnsupportedUnit)?;

            match settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let request = MintQuoteBolt11Request {
            amount,
            unit: unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
        };

        let quote_res = self.client.post_mint_quote(request).await?;

        let quote = MintQuote {
            mint_url: mint_url.clone(),
            id: quote_res.quote.clone(),
            amount,
            unit: unit.clone(),
            request: quote_res.request.clone(),
            state: quote_res.state,
            expiry: quote_res.expiry.unwrap_or(0),
            secret_key: Some(secret_key),
        };

        self.localstore.add_mint_quote(quote.clone()).await?;

        let mut metadata = HashMap::new();
        metadata.insert("quote_id".to_string(), quote_res.quote);

        let tx = Transaction {
            mint_url,
            direction: TransactionDirection::Incoming,
            kind: TransactionKind::LN,
            amount,
            fee: Amount::ZERO,
            unit,
            ys: vec![quote_res.pubkey.unwrap()],
            token: quote_res.request,
            status: cdk_common::wallet::TransactionStatus::Pending,
            timestamp: unix_time(),
            memo: None,
            metadata,
        };
        // Add transaction to store
        self.localstore.add_transaction(tx.clone()).await?;

        Ok((quote, tx))
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let response = self.client.get_mint_quote_status(quote_id).await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.state = response.state;
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Check status of pending mint quotes
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            let mint_quote_response = self.mint_quote_state(&mint_quote.id).await?;

            if mint_quote_response.state == MintQuoteState::Paid {
                let proofs = self
                    .mint(&mint_quote.id, SplitTarget::default(), None)
                    .await?;
                total_amount += proofs.0.total_amount()?;
            } else if mint_quote.expiry.le(&unix_time()) {
                self.localstore.remove_mint_quote(&mint_quote.id).await?;
            }
        }
        Ok(total_amount)
    }

    /// Mint
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use anyhow::Result;
    /// use cdk::amount::{Amount, SplitTarget};
    /// use cdk::nuts::nut00::ProofsMethods;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let seed = random::<[u8; 32]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None).unwrap();
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     let quote_id = quote.id;
    ///     // To be called after quote request is paid
    ///     let minted_proofs = wallet.mint(&quote_id, SplitTarget::default(), None).await?;
    ///     let minted_amount = minted_proofs.total_amount()?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<(Proofs, Transaction), Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let quote_info = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let unix_time = unix_time();

        if quote_info.expiry > unix_time {
            tracing::warn!("Attempting to mint with expired quote.");
        }

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let count = count.map_or(0, |c| c + 1);

        let premint_secrets = match &spending_conditions {
            Some(spending_conditions) => PreMintSecrets::with_conditions(
                active_keyset_id,
                quote_info.amount,
                &amount_split_target,
                spending_conditions,
            )?,
            None => PreMintSecrets::from_xpriv(
                active_keyset_id,
                count,
                self.xpriv,
                quote_info.amount,
                &amount_split_target,
            )?,
        };

        let mut request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };

        if let Some(secret_key) = quote_info.secret_key {
            request.sign(secret_key)?;
        }

        let mut tx = self
            .list_pending_transactions()
            .await?
            .into_iter()
            .find(|t| {
                t.kind == TransactionKind::LN
                    && t.direction == TransactionDirection::Incoming
                    && t.metadata.get("quote_id") == Some(&quote_id.to_string())
            })
            .ok_or(Error::TransactionNotFound)?;

        // let mint_res = self.client.post_mint(request).await?;
        // Set a timeout for the mint request
        let mint_res = timeout(Duration::from_secs(15), self.client.post_mint(request)).await;
        let mint_res = match mint_res {
            Ok(Ok(res)) => res,
            Ok(Err(err)) => {
                tracing::error!("post_mint failed: {}", err);
                tx.status = cdk_common::wallet::TransactionStatus::Failed;
                self.localstore.add_transaction(tx.clone()).await?;
                return Err(err);
            }
            Err(_) => {
                tracing::warn!("post_mint timed out after 15s");
                tx.status = cdk_common::wallet::TransactionStatus::Failed;
                self.localstore.add_transaction(tx.clone()).await?;
                return Err(Error::Timeout);
            }
        };

        let keys = self.get_keyset_keys(active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                let keys = self.get_keyset_keys(sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
                match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                    Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
        }

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        // Remove filled quote from store
        self.localstore.remove_mint_quote(&quote_info.id).await?;

        if spending_conditions.is_none() {
            tracing::debug!(
                "Incrementing keyset {} counter by {}",
                active_keyset_id,
                proofs.len()
            );

            // Update counter for keyset
            self.localstore
                .increment_keyset_counter(&active_keyset_id, proofs.len() as u32)
                .await?;
        }

        let proof_infos = proofs
            .iter()
            .map(|proof| {
                ProofInfo::new(
                    proof.clone(),
                    self.mint_url.clone(),
                    State::Unspent,
                    quote_info.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Add new proofs to store
        self.localstore.update_proofs(proof_infos, vec![]).await?;

        // update transaction info
        tx.amount = proofs.total_amount()?;
        tx.timestamp = unix_time;
        // tx.ys = proofs.ys()?;
        tx.status = cdk_common::wallet::TransactionStatus::Success;

        // have add transaction in fn request_mint,
        // then will update in checking mint quote
        Ok((proofs, tx))
    }
}
