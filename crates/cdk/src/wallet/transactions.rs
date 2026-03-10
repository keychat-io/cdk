use std::str::FromStr;

use cdk_common::mint_url::MintUrl;
use cdk_common::wallet::{
    Transaction, TransactionDirection, TransactionId, TransactionKind, TransactionStatus,
};
use cdk_common::Proofs;

use crate::{Error, Wallet};

impl Wallet {
    /// List transactions
    pub async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions(
                Some(self.mint_url.clone()),
                direction,
                Some(self.unit.clone()),
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// List transactions with status
    pub async fn list_transactions_with_status(
        &self,
        direction: Option<TransactionDirection>,
        status: TransactionStatus,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions_with_status(
                Some(self.mint_url.clone()),
                direction,
                Some(self.unit.clone()),
                status,
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// List transactions with kind and offset
    pub async fn list_transactions_with_kind_offset(
        &self,
        offset: usize,
        limit: usize,
        kind: &[TransactionKind],
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions_with_kind_offset(
                offset,
                limit,
                kind,
                Some(self.mint_url.clone()),
                direction,
                Some(self.unit.clone()),
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// List transactions with kind and offset for a specific mint
    pub async fn list_transactions_with_kind_offset_mint(
        &self,
        offset: usize,
        limit: usize,
        mint_url: &str,
        kind: &[TransactionKind],
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions_with_kind_offset(
                offset,
                limit,
                kind,
                Some(MintUrl::from_str(mint_url)?),
                direction,
                Some(self.unit.clone()),
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// List transactions with kind and amount filter and offset
    pub async fn list_transactions_with_kind_amount_offset(
        &self,
        offset: usize,
        limit: usize,
        mint_url: &str,
        kind: &[TransactionKind],
        direction: Option<TransactionDirection>,
        amount: Option<i64>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions_with_kind_amount_offset(
                offset,
                limit,
                kind,
                Some(MintUrl::from_str(mint_url)?),
                direction,
                Some(self.unit.clone()),
                amount,
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// List pending transactions
    pub async fn list_pending_transactions(&self) -> Result<Vec<Transaction>, Error> {
        self.list_transactions_with_status(None, TransactionStatus::Pending)
            .await
    }

    /// List failed transactions
    pub async fn list_failed_transactions(&self) -> Result<Vec<Transaction>, Error> {
        self.list_transactions_with_status(None, TransactionStatus::Failed)
            .await
    }

    /// List pending and failed transactions
    pub async fn list_pending_failed_transactions(&self) -> Result<Vec<Transaction>, Error> {
        let pending_txs = self.list_pending_transactions().await?;
        let failed_txs = self.list_failed_transactions().await?;
        let mut result = Vec::new();
        result.extend(pending_txs);
        result.extend(failed_txs);
        Ok(result)
    }

    /// Remove transactions older than the given timestamp
    pub async fn remove_transactions(&self, unix_timestamp_le: u64) -> Result<(), Error> {
        self.localstore
            .remove_transactions(unix_timestamp_le)
            .await?;
        Ok(())
    }

    /// Get transaction by ID
    pub async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Error> {
        let transaction = self.localstore.get_transaction(id).await?;

        Ok(transaction)
    }

    /// Get proofs for a transaction by transaction ID
    ///
    /// This retrieves all proofs associated with a transaction by looking up
    /// the transaction's Y values and fetching the corresponding proofs.
    pub async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Error> {
        let transaction = self
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        let proofs = self
            .localstore
            .get_proofs_by_ys(transaction.ys)
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect();

        Ok(proofs)
    }

    /// Revert a transaction by reclaiming unspent proofs.
    ///
    /// For transactions created by the saga pattern (with `saga_id` set), this
    /// function loads the associated send saga and calls `revoke()` on it, which
    /// properly handles the saga lifecycle including state transitions and cleanup.
    ///
    /// For legacy transactions (without `saga_id`), this function checks the proofs
    /// with the mint and marks any spent proofs accordingly. Unspent proofs are
    /// left in their current state for manual recovery.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The transaction is not found
    /// - The transaction is not outgoing split
    /// - The saga is not in a revocable state (e.g., already completed)
    /// - The token has already been claimed by the recipient
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<(), Error> {
        let tx = self
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if tx.direction != TransactionDirection::Outgoing
            && tx.direction != TransactionDirection::Split
        {
            return Err(Error::InvalidTransactionDirection);
        }

        // Check if this is a saga-managed transaction
        if let Some(saga_id) = &tx.saga_id {
            // Use the existing revoke_send method which properly handles the saga
            // Discard the returned amount - we just care about success/failure
            let _ = self.revoke_send(*saga_id).await?;
            Ok(())
        } else {
            // Legacy transaction without saga - check proofs and mark spent ones
            // We don't attempt to swap for legacy transactions to avoid
            // interfering with any potential in-flight operations
            let pending_spent_proofs: Proofs = self
                .get_pending_spent_proofs()
                .await?
                .into_iter()
                .filter(|p| match p.y() {
                    Ok(y) => tx.ys.contains(&y),
                    Err(_) => false,
                })
                .collect();

            if pending_spent_proofs.is_empty() {
                return Ok(());
            }

            // Just check and mark spent - don't attempt swap for legacy transactions
            self.check_proofs_spent(pending_spent_proofs).await?;
            Ok(())
        }
    }
}
