//! Swap module for the wallet.
//!
//! This module provides functionality for swapping proofs.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk_common::amount::FeeAndAmounts;
use cdk_common::wallet::{Transaction, TransactionDirection, TransactionKind, TransactionStatus};
use cdk_common::Id;
use tokio::time::timeout;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::fees::ProofsFeeBreakdown;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut10, CheckStateRequest, PreMintSecrets, PreSwap, Proofs, PublicKey, SpendingConditions,
    State, SwapRequest, Token,
};
use crate::types::ProofInfo;
use crate::{Amount, Error, Wallet};

pub(crate) mod saga;

use saga::SwapSaga;

impl Wallet {
    async fn reclaim_or_record_tx(
        &self,
        input_proofs: Proofs,
        tx: Transaction,
    ) -> Result<(), Error> {
        // Try to reclaim by checking which proofs are unspent and swapping them back
        let spendable = self
            .client
            .post_check_state(CheckStateRequest {
                ys: input_proofs.ys()?,
            })
            .await;

        match spendable {
            Ok(response) => {
                let unspent: Proofs = input_proofs
                    .into_iter()
                    .zip(response.states.iter())
                    .filter_map(|(p, s)| (s.state == State::Unspent).then_some(p))
                    .collect();

                if unspent.is_empty() {
                    tracing::warn!("No unspent proofs to reclaim");
                    return Ok(());
                }
                tracing::info!("Reclaiming {} unspent proofs", unspent.len());
                self.swap(None, SplitTarget::default(), unspent, None, false)
                    .await?;
                Ok(())
            }
            Err(err) => {
                tracing::error!("Could not reclaim unspent proofs: {}", err);
                self.localstore.add_transaction(tx).await?;
                Err(err)
            }
        }
    }

    /// Swap with denomination
    #[instrument(skip(self, input_proofs))]
    pub async fn swap_denomination(
        &self,
        denomination: Amount,
        amount: Option<Amount>,
        input_proofs: Proofs,
        include_fees: bool,
    ) -> Result<Option<Proofs>, Error> {
        tracing::info!("Swapping denomination");
        let mint_url = &self.mint_url;
        let unit = &self.unit;
        let token = Token::new(mint_url.clone(), input_proofs.clone(), None, unit.clone());

        let pre_swap = self
            .create_swap_denomination(denomination, amount, input_proofs.clone(), include_fees)
            .await?;
        let fee = pre_swap.fee;

        let mut tx = Transaction {
            mint_url: mint_url.clone(),
            direction: TransactionDirection::Split,
            kind: TransactionKind::Cashu,
            amount: 32.into(),
            fee,
            unit: unit.clone(),
            ys: input_proofs.ys()?,
            token: token.to_string(),
            status: TransactionStatus::Failed,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: None,
        };

        let swap_response = timeout(
            tokio::time::Duration::from_secs(15),
            self.client.post_swap(pre_swap.swap_request),
        )
        .await;

        let swap_response = match swap_response {
            Ok(Ok(res)) => res,
            Ok(Err(err)) => {
                tracing::error!("post_swap failed: {}", err);
                self.reclaim_or_record_tx(input_proofs.clone(), tx.clone())
                    .await?;
                return Err(err);
            }
            Err(_) => {
                tracing::error!("post_swap timed out after 15s");
                self.reclaim_or_record_tx(input_proofs.clone(), tx.clone())
                    .await?;
                return Err(Error::Timeout);
            }
        };

        let active_keyset_id = pre_swap.pre_mint_secrets.keyset_id;

        let active_keys = self
            .localstore
            .get_keys(&active_keyset_id)
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        self.localstore
            .increment_keyset_counter(&active_keyset_id, pre_swap.derived_secret_count)
            .await?;

        let mut added_proofs = Vec::new();
        let change_proofs;
        let send_proofs;
        match amount {
            Some(amount) => {
                let (_proofs_with_condition, proofs_without_condition): (Proofs, Proofs) =
                    post_swap_proofs.into_iter().partition(|p| {
                        let nut10_secret: Result<nut10::Secret, _> = p.secret.clone().try_into();
                        nut10_secret.is_ok()
                    });

                let (proofs_to_send, proofs_to_keep) = {
                    let mut all_proofs = proofs_without_condition;
                    all_proofs.reverse();

                    let mut proofs_to_send = Proofs::new();
                    let mut proofs_to_keep = Proofs::new();
                    let target = vec![denomination; *amount.as_ref() as usize];
                    let split_target = SplitTarget::Values(target);
                    let mut amount_split = amount.split_targeted(
                        &split_target,
                        &(0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
                    )?;

                    for proof in all_proofs {
                        if let Some(idx) = amount_split.iter().position(|&a| a == proof.amount) {
                            proofs_to_send.push(proof);
                            amount_split.remove(idx);
                        } else {
                            proofs_to_keep.push(proof);
                        }
                    }

                    (proofs_to_send, proofs_to_keep)
                };

                let send_proofs_info = proofs_to_send
                    .clone()
                    .into_iter()
                    .map(|proof| {
                        ProofInfo::new(proof, mint_url.clone(), State::Unspent, unit.clone())
                    })
                    .collect::<Result<Vec<ProofInfo>, _>>()?;
                added_proofs = send_proofs_info;

                change_proofs = proofs_to_keep;
                send_proofs = Some(proofs_to_send);
            }
            None => {
                change_proofs = post_swap_proofs;
                send_proofs = None;
            }
        }

        let keep_proofs = change_proofs
            .into_iter()
            .map(|proof| ProofInfo::new(proof, mint_url.clone(), State::Unspent, unit.clone()))
            .collect::<Result<Vec<ProofInfo>, _>>()?;
        added_proofs.extend(keep_proofs);

        let deleted_ys = input_proofs
            .into_iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<PublicKey>, _>>()?;

        self.localstore
            .update_proofs(added_proofs, deleted_ys)
            .await?;

        tx.status = TransactionStatus::Success;
        self.localstore.add_transaction(tx).await?;
        Ok(send_proofs)
    }
    /// Swap proofs using the saga pattern
    #[instrument(skip(self, input_proofs))]
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, Error> {
        tracing::info!("Swapping");

        let saga = SwapSaga::new(self);
        let saga = saga
            .prepare(
                amount,
                amount_split_target,
                input_proofs,
                spending_conditions,
                include_fees,
            )
            .await?;
        let saga = saga.execute().await?;

        Ok(saga.into_send_proofs())
    }

    /// Create Swap Payload
    #[instrument(skip(self, proofs))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_swap(
        &self,
        active_keyset_id: Id,
        fee_and_amounts: &FeeAndAmounts,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        proofs_fee_breakdown: &ProofsFeeBreakdown,
    ) -> Result<PreSwap, Error> {
        tracing::info!("Creating swap");

        // Desired amount is either amount passed or value of all proof
        let proofs_total = proofs.total_amount()?;

        let ys: Vec<PublicKey> = proofs.ys()?;
        self.localstore
            .update_proofs_state(ys, State::Reserved)
            .await?;

        let total_to_subtract = amount
            .unwrap_or(Amount::ZERO)
            .checked_add(proofs_fee_breakdown.total)
            .ok_or(Error::AmountOverflow)?;

        let change_amount: Amount = proofs_total
            .checked_sub(total_to_subtract)
            .ok_or(Error::InsufficientFunds)?;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), fee_and_amounts)?
                    .len();

                let fee_to_redeem = self
                    .get_keyset_count_fee(&active_keyset_id, split_count as u64)
                    .await?;

                (
                    amount
                        .map(|a| a.checked_add(fee_to_redeem).ok_or(Error::AmountOverflow))
                        .transpose()?,
                    change_amount
                        .checked_sub(fee_to_redeem)
                        .ok_or(Error::InsufficientFunds)?,
                )
            }
            false => (amount, change_amount),
        };

        // If a non None split target is passed use that
        // else use state refill
        let change_split_target = match amount_split_target {
            SplitTarget::None => {
                self.determine_split_target_values(change_amount, fee_and_amounts)
                    .await?
            }
            s => s,
        };

        let derived_secret_count;

        // Calculate total secrets needed and atomically reserve counter range
        let total_secrets_needed = match spending_conditions {
            Some(_) => {
                // For spending conditions, we only need to count change secrets
                change_amount
                    .split_targeted(&change_split_target, fee_and_amounts)?
                    .len() as u32
            }
            None => {
                // For no spending conditions, count both send and change secrets
                let send_count = send_amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), fee_and_amounts)?
                    .len() as u32;
                let change_count = change_amount
                    .split_targeted(&change_split_target, fee_and_amounts)?
                    .len() as u32;
                send_count + change_count
            }
        };

        // Atomically get the counter range we need
        let starting_counter = if total_secrets_needed > 0 {
            tracing::debug!(
                "Incrementing keyset {} counter by {}",
                active_keyset_id,
                total_secrets_needed
            );

            let new_counter = self
                .localstore
                .increment_keyset_counter(&active_keyset_id, total_secrets_needed)
                .await?;

            new_counter - total_secrets_needed
        } else {
            0
        };

        let mut count = starting_counter;

        let (mut desired_messages, change_messages) = match spending_conditions {
            Some(conditions) => {
                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len();

                (
                    PreMintSecrets::with_conditions(
                        active_keyset_id,
                        send_amount.unwrap_or(Amount::ZERO),
                        &SplitTarget::default(),
                        &conditions,
                        fee_and_amounts,
                    )?,
                    change_premint_secrets,
                )
            }
            None => {
                let premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    send_amount.unwrap_or(Amount::ZERO),
                    &SplitTarget::default(),
                    fee_and_amounts,
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len() + premint_secrets.len();

                (premint_secrets, change_premint_secrets)
            }
        };

        // Combine the BlindedMessages totaling the desired amount with change
        desired_messages.combine(change_messages);
        // Sort the premint secrets to avoid finger printing
        desired_messages.sort_secrets();

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
            derived_secret_count: derived_secret_count as u32,
            fee: proofs_fee_breakdown.total,
        })
    }

    /// Create Swap Payload with denomination
    #[instrument(skip(self, proofs))]
    pub async fn create_swap_denomination(
        &self,
        denomination: Amount,
        amount: Option<Amount>,
        proofs: Proofs,
        include_fees: bool,
    ) -> Result<PreSwap, Error> {
        tracing::info!("Creating swap denomination");
        let active_keyset_id = self.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let proofs_total = proofs.total_amount()?;

        let ys: Vec<PublicKey> = proofs.ys()?;
        self.localstore
            .update_proofs_state(ys, State::Reserved)
            .await?;

        let fee = self.get_proofs_fee(&proofs).await?;

        let change_amount: Amount = proofs_total - amount.unwrap_or(Amount::ZERO) - fee.total;

        let change_split_target = self
            .determine_split_target_values(change_amount, &fee_and_amounts)
            .await?;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), &fee_and_amounts)?
                    .len();

                let fee_to_redeem = self
                    .get_keyset_count_fee(&active_keyset_id, split_count as u64)
                    .await?;

                (
                    amount.map(|a| a + fee_to_redeem),
                    change_amount - fee_to_redeem,
                )
            }
            false => (amount, change_amount),
        };

        let derived_secret_count;

        // Calculate total secrets needed for atomic counter reservation
        let send_count = *send_amount.unwrap_or(Amount::ZERO).as_ref() as u32;
        let change_count = change_amount
            .split_targeted(&change_split_target, &fee_and_amounts)?
            .len() as u32;
        let total_secrets_needed = send_count + change_count;

        let starting_counter = if total_secrets_needed > 0 {
            let new_counter = self
                .localstore
                .increment_keyset_counter(&active_keyset_id, total_secrets_needed)
                .await?;
            new_counter - total_secrets_needed
        } else {
            0
        };

        let mut count = starting_counter;

        let (mut desired_messages, change_messages) = {
            let premint_secrets = PreMintSecrets::from_seed_denomination(
                active_keyset_id,
                count,
                &self.seed,
                send_amount.unwrap_or(Amount::ZERO),
                denomination,
            )?;

            count += premint_secrets.len() as u32;

            let change_premint_secrets = PreMintSecrets::from_seed(
                active_keyset_id,
                count,
                &self.seed,
                change_amount,
                &change_split_target,
                &fee_and_amounts,
            )?;

            derived_secret_count = change_premint_secrets.len() + premint_secrets.len();

            (premint_secrets, change_premint_secrets)
        };

        desired_messages.combine(change_messages);
        desired_messages.sort_secrets();

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
            derived_secret_count: derived_secret_count as u32,
            fee: fee.total,
        })
    }
}
