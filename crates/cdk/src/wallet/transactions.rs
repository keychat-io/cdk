use cdk_common::wallet::{
    Transaction, TransactionDirection, TransactionId, TransactionKind, TransactionStatus,
};

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

    /// list transactions with kind and offset
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

    /// list pending transactions with status
    pub async fn list_pending_transactions(&self) -> Result<Vec<Transaction>, Error> {
        let all_txs = self.list_transactions(None).await?;
        // let all_pending_proofs = self.get_all_pending_proofs().await?;
        // println!("all_pending_proofs {:?}", all_pending_proofs);
        // find all pending_txs
        // let pending_txs = all_txs
        //     .into_iter()
        //     .filter(|tx| {
        //         all_pending_proofs.iter().any(|p| match p.y() {
        //             Ok(y) => tx.ys.contains(&y),
        //             Err(_) => false,
        //         })
        //     })
        //     .collect::<Vec<_>>();
        let pending_txs = all_txs
            .into_iter()
            .filter(|tx| tx.status == TransactionStatus::Pending)
            .collect();
        Ok(pending_txs)
    }

    /// list pending failed transactions with status
    pub async fn list_pending_failed_transactions(&self) -> Result<Vec<Transaction>, Error> {
        let all_txs = self.list_transactions(None).await?;

        let pending_or_failed_txs = all_txs
            .into_iter()
            .filter(|tx| {
                matches!(
                    tx.status,
                    TransactionStatus::Pending | TransactionStatus::Failed
                )
            })
            .collect();
        Ok(pending_or_failed_txs)
    }

    /// Get transaction by ID
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

    /// Revert a transaction
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<(), Error> {
        let tx = self
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if tx.direction != TransactionDirection::Outgoing {
            return Err(Error::InvalidTransactionDirection);
        }

        let pending_spent_proofs = self
            .get_pending_spent_proofs()
            .await?
            .into_iter()
            .filter(|p| match p.y() {
                Ok(y) => tx.ys.contains(&y),
                Err(_) => false,
            })
            .collect::<Vec<_>>();

        self.reclaim_unspent(pending_spent_proofs).await?;
        Ok(())
    }
}
