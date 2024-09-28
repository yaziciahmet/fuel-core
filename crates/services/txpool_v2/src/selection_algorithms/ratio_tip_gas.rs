use std::{
    cmp::{
        Ordering,
        Reverse,
    },
    collections::{
        BTreeMap,
        HashMap,
    },
    fmt::Debug,
    time::Instant,
};

use fuel_core_types::{
    fuel_tx::TxId,
    services::txpool::PoolTransaction,
};
use num_rational::Ratio;

use crate::{
    error::Error,
    storage::StorageData,
};

use super::{
    Constraints,
    SelectionAlgorithm,
};

pub trait RatioTipGasSelectionAlgorithmStorage {
    type StorageIndex: Copy + Debug;

    fn get(&self, index: &Self::StorageIndex) -> Option<&StorageData>;

    fn get_dependents(
        &self,
        index: &Self::StorageIndex,
    ) -> impl Iterator<Item = Self::StorageIndex>;
}

pub type RatioTipGas = Ratio<u64>;

/// Key used to sort transactions by tip/gas ratio.
/// It first compares the tip/gas ratio, then the creation instant and finally the transaction id.
#[derive(Eq, PartialEq, Clone, Copy, Debug)]
pub struct Key {
    ratio: RatioTipGas,
    creation_instant: Instant,
    tx_id: TxId,
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        let cmp = self.ratio.cmp(&other.ratio);
        if cmp == Ordering::Equal {
            let instant_cmp = other.creation_instant.cmp(&self.creation_instant);
            if instant_cmp == Ordering::Equal {
                self.tx_id.cmp(&other.tx_id)
            } else {
                instant_cmp
            }
        } else {
            cmp
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The selection algorithm that selects transactions based on the tip/gas ratio.
pub struct RatioTipGasSelection<S: RatioTipGasSelectionAlgorithmStorage> {
    executable_transactions_sorted_tip_gas_ratio: BTreeMap<Reverse<Key>, S::StorageIndex>,
    tx_id_to_creation_instant: HashMap<TxId, Instant>,
}

impl<S: RatioTipGasSelectionAlgorithmStorage> RatioTipGasSelection<S> {
    pub fn new() -> Self {
        Self {
            executable_transactions_sorted_tip_gas_ratio: BTreeMap::new(),
            tx_id_to_creation_instant: HashMap::new(),
        }
    }

    fn on_stored_transaction_inner(&mut self, store_entry: &StorageData) -> Key {
        let transaction = &store_entry.transaction;
        let tip_gas_ratio = RatioTipGas::new(transaction.tip(), transaction.max_gas());
        let key = Key {
            ratio: tip_gas_ratio,
            creation_instant: store_entry.creation_instant,
            tx_id: transaction.id(),
        };
        self.tx_id_to_creation_instant
            .insert(transaction.id(), store_entry.creation_instant);
        key
    }

    fn on_removed_transaction_inner(&mut self, ratio: RatioTipGas, tx_id: TxId) {
        let creation_instant = self.tx_id_to_creation_instant.remove(&tx_id);

        if let Some(creation_instant) = creation_instant {
            let key = Key {
                ratio,
                creation_instant,
                tx_id,
            };
            self.executable_transactions_sorted_tip_gas_ratio
                .remove(&Reverse(key));
        }
    }
}

impl<S: RatioTipGasSelectionAlgorithmStorage> Default for RatioTipGasSelection<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: RatioTipGasSelectionAlgorithmStorage> SelectionAlgorithm
    for RatioTipGasSelection<S>
{
    type Storage = S;
    type StorageIndex = S::StorageIndex;

    fn gather_best_txs(
        &mut self,
        constraints: Constraints,
        storage: &S,
    ) -> Result<Vec<S::StorageIndex>, Error> {
        let mut gas_left = constraints.max_gas;
        let mut result = Vec::new();

        // Take iterate over all transactions with the highest tip/gas ratio. If transaction
        // fits in the gas limit select it and mark all its dependents to be promoted.
        // Do that until end of the list or gas limit is reached. If gas limit is not
        // reached, but we have promoted transactions we can start again from the beginning.
        // Otherwise, we can break the loop.
        // It is done in this way to minimize number of iteration of the list of executable
        // transactions.
        while gas_left > 0
            && !self.executable_transactions_sorted_tip_gas_ratio.is_empty()
        {
            let mut best_transactions = Vec::new();
            let mut transactions_to_remove = Vec::new();
            let mut transactions_to_promote = Vec::new();

            let sorted_iter = self.executable_transactions_sorted_tip_gas_ratio.iter();
            for (key, storage_id) in sorted_iter {
                let Some(stored_transaction) = storage.get(storage_id) else {
                    debug_assert!(
                        false,
                        "Transaction not found in the storage during `gather_best_txs`."
                    );
                    tracing::warn!(
                        "Transaction not found in the storage during `gather_best_txs`."
                    );
                    transactions_to_remove.push(*key);
                    continue
                };

                if stored_transaction.transaction.max_gas() > gas_left {
                    continue;
                }

                gas_left =
                    gas_left.saturating_sub(stored_transaction.transaction.max_gas());
                best_transactions.push((*key, *storage_id));

                transactions_to_promote.extend(storage.get_dependents(storage_id));
            }

            for remove in transactions_to_remove {
                let key = remove.0;
                self.on_removed_transaction_inner(key.ratio, key.tx_id);
            }

            // If no transaction fits in the gas limit and no one to promote, we can break the loop
            if best_transactions.is_empty() && transactions_to_promote.is_empty() {
                break;
            }

            for (key, storage_id) in best_transactions {
                let key = key.0;
                // Remove the best transaction from the sorted list
                self.on_removed_transaction_inner(key.ratio, key.tx_id);
                result.push(storage_id);
            }

            for promote in transactions_to_promote {
                let storage = storage.get(&promote).expect(
                    "We just get the dependent from the storage, it should exist.",
                );

                self.new_executable_transaction(promote, storage);
            }
        }

        Ok(result)
    }

    fn new_executable_transaction(
        &mut self,
        storage_id: Self::StorageIndex,
        store_entry: &StorageData,
    ) {
        let key = self.on_stored_transaction_inner(store_entry);
        self.executable_transactions_sorted_tip_gas_ratio
            .insert(Reverse(key), storage_id);
    }

    fn get_less_worth_txs(&self) -> impl Iterator<Item = &Self::StorageIndex> {
        self.executable_transactions_sorted_tip_gas_ratio
            .values()
            .rev()
    }

    fn on_stored_transaction(&mut self, store_entry: &StorageData) {
        self.on_stored_transaction_inner(store_entry);
    }

    fn on_removed_transaction(&mut self, transaction: &PoolTransaction) {
        let tip_gas_ratio = RatioTipGas::new(transaction.tip(), transaction.max_gas());
        let tx_id = transaction.id();
        self.on_removed_transaction_inner(tip_gas_ratio, tx_id)
    }
}
