use crate::{
    fuel_core_graphql_api::{
        ports::{
            DatabaseBlocks,
            DatabaseMessageProof,
            DatabaseMessages,
            OffChainDatabase,
            OnChainDatabase,
        },
        IntoApiResult,
    },
    query::{
        SimpleBlockData,
        SimpleTransactionData,
        TransactionQueryData,
    },
};
use fuel_core_storage::{
    iter::{
        BoxedIter,
        IntoBoxedIter,
        IterDirection,
    },
    not_found,
    tables::Messages,
    Error as StorageError,
    Result as StorageResult,
    StorageAsRef,
};
use fuel_core_types::{
    blockchain::block::CompressedBlock,
    entities::relayer::message::{
        MerkleProof,
        Message,
        MessageProof,
        MessageStatus,
    },
    fuel_merkle::binary::in_memory::MerkleTree,
    fuel_tx::{
        input::message::compute_message_id,
        Receipt,
        TxId,
    },
    fuel_types::{
        Address,
        BlockHeight,
        Bytes32,
        MessageId,
        Nonce,
    },
    services::txpool::TransactionStatus,
};
use itertools::Itertools;
use std::borrow::Cow;

#[cfg(test)]
mod test;

pub trait MessageQueryData: Send + Sync {
    fn message(&self, message_id: &Nonce) -> StorageResult<Message>;

    fn owned_message_ids(
        &self,
        owner: &Address,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Nonce>>;

    fn owned_messages(
        &self,
        owner: &Address,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Message>>;

    fn all_messages(
        &self,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Message>>;
}

impl<D: OnChainDatabase + OffChainDatabase + ?Sized> MessageQueryData for D {
    fn message(&self, id: &Nonce) -> StorageResult<Message> {
        self.storage::<Messages>()
            .get(id)?
            .ok_or(not_found!(Messages))
            .map(Cow::into_owned)
    }

    fn owned_message_ids(
        &self,
        owner: &Address,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Nonce>> {
        self.owned_message_ids(owner, start_message_id, direction)
    }

    fn owned_messages(
        &self,
        owner: &Address,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Message>> {
        self.owned_message_ids(owner, start_message_id, direction)
            .map(|result| result.and_then(|id| self.message(&id)))
            .into_boxed()
    }

    fn all_messages(
        &self,
        start_message_id: Option<Nonce>,
        direction: IterDirection,
    ) -> BoxedIter<StorageResult<Message>> {
        self.all_messages(start_message_id, direction)
    }
}

/// Trait that specifies all the data required by the output message query.
pub trait MessageProofData:
    Send + Sync + SimpleBlockData + SimpleTransactionData + DatabaseMessageProof
{
    /// Get the status of a transaction.
    fn transaction_status(
        &self,
        transaction_id: &TxId,
    ) -> StorageResult<TransactionStatus>;
}

impl<D> MessageProofData for D
where
    D: OnChainDatabase + DatabaseBlocks + OffChainDatabase + ?Sized,
{
    fn transaction_status(
        &self,
        transaction_id: &TxId,
    ) -> StorageResult<TransactionStatus> {
        self.status(transaction_id)
    }
}

/// Generate an output proof.
pub fn message_proof<T: MessageProofData + ?Sized>(
    database: &T,
    transaction_id: Bytes32,
    desired_nonce: Nonce,
    commit_block_height: BlockHeight,
) -> StorageResult<MessageProof> {
    // Check if the receipts for this transaction actually contain this nonce or exit.
    let (sender, recipient, nonce, amount, data) = database
        .receipts(&transaction_id)?
        .into_iter()
        .find_map(|r| match r {
            Receipt::MessageOut {
                sender,
                recipient,
                nonce,
                amount,
                data,
                ..
            } if r.nonce() == Some(&desired_nonce) => {
                Some((sender, recipient, nonce, amount, data))
            }
            _ => None,
        })
        .ok_or::<StorageError>(
            anyhow::anyhow!("Desired `nonce` missing in transaction receipts").into(),
        )?;

    let Some(data) = data else {
        return Err(anyhow::anyhow!("Output message doesn't contain any `data`").into())
    };

    // Get the block id from the transaction status if it's ready.
    let Some(TransactionStatus::Success {
        block_height: message_block_height,
        ..
    }) = database
        .transaction_status(&transaction_id)
        .into_api_result::<TransactionStatus, StorageError>()?
    else {
        return Err(anyhow::anyhow!("Unable to obtain the message block height").into())
    };

    // Get the message fuel block header.
    let Some(message_block) = database
        .block(&message_block_height)
        .into_api_result::<CompressedBlock, StorageError>()?
    else {
        return Err(
            anyhow::anyhow!("Unable to get the message block from the database").into(),
        )
    };
    let (message_block_header, message_block_txs) = message_block.into_inner();

    let message_id = compute_message_id(&sender, &recipient, &nonce, amount, &data);

    let message_proof = message_receipts_proof(database, message_id, &message_block_txs)?;

    // Get the commit fuel block header.
    let Some(commit_block_header) = database
        .block(&commit_block_height)
        .into_api_result::<CompressedBlock, StorageError>()?
    else {
        return Err(
            anyhow::anyhow!("Unable to get commit block header from database").into(),
        )
    };
    let (commit_block_header, _) = commit_block_header.into_inner();

    let Some(verifiable_commit_block_height) = commit_block_header.height().pred() else {
        return Err(anyhow::anyhow!(
            "Impossible to generate proof beyond the genesis block"
        )
        .into())
    };
    let block_proof = database.block_history_proof(
        message_block_header.height(),
        &verifiable_commit_block_height,
    )?;

    Ok(MessageProof {
        message_proof,
        block_proof,
        message_block_header,
        commit_block_header,
        sender,
        recipient,
        nonce,
        amount,
        data,
    })
}

fn message_receipts_proof<T: MessageProofData + ?Sized>(
    database: &T,
    message_id: MessageId,
    message_block_txs: &[Bytes32],
) -> StorageResult<MerkleProof> {
    // Get the message receipts from the block.
    let leaves: Vec<Vec<Receipt>> = message_block_txs
        .iter()
        .map(|id| database.receipts(id))
        .filter_map(|result| result.into_api_result::<_, StorageError>().transpose())
        .try_collect()?;
    let leaves = leaves.into_iter()
        // Flatten the receipts after filtering on output messages
        // and mapping to message ids.
        .flat_map(|receipts|
            receipts.into_iter().filter_map(|r| r.message_id()));

    // Build the merkle proof from the above iterator.
    let mut tree = MerkleTree::new();

    let mut proof_index = None;

    for (index, id) in leaves.enumerate() {
        // Check if this is the message id being proved.
        if message_id == id {
            // Save the index of this message to use as the proof index.
            proof_index = Some(index as u64);
        }

        // Build the merkle tree.
        tree.push(id.as_ref());
    }

    // Check if we found a leaf.
    let Some(proof_index) = proof_index else {
        return Err(anyhow::anyhow!(
            "Unable to find the message receipt in the transaction to generate the proof"
        )
        .into())
    };

    // Get the proof set.
    let Some((_, proof_set)) = tree.prove(proof_index) else {
        return Err(anyhow::anyhow!(
            "Unable to generate the Merkle proof for the message from its receipts"
        )
        .into());
    };

    // Return the proof.
    Ok(MerkleProof {
        proof_set,
        proof_index,
    })
}

pub fn message_status<T>(
    database: &T,
    message_nonce: Nonce,
) -> StorageResult<MessageStatus>
where
    T: OffChainDatabase + DatabaseMessages + ?Sized,
{
    if database.message_is_spent(&message_nonce)? {
        Ok(MessageStatus::spent())
    } else if database.message_exists(&message_nonce)? {
        Ok(MessageStatus::unspent())
    } else {
        Ok(MessageStatus::not_found())
    }
}
