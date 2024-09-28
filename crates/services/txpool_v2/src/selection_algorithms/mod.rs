use std::time::Instant;

use fuel_core_types::services::txpool::PoolTransaction;

use crate::{
    error::Error,
    storage::{
        Storage,
        StorageData,
    },
};

pub mod ratio_tip_gas;

/// Constraints that the selection algorithm has to respect.
pub struct Constraints {
    pub max_gas: u64,
}

/// The selection algorithm is responsible for selecting the best transactions to include in a block.
pub trait SelectionAlgorithm {
    /// The storage type of the selection algorithm.
    type Storage;
    /// The index that identifies a transaction in the storage.
    type StorageIndex;
    /// Given the constraints, the selection algorithm has to return the best list of transactions to include in a block.
    fn gather_best_txs(
        &mut self,
        constraints: Constraints,
        storage: &Self::Storage,
    ) -> Result<Vec<Self::StorageIndex>, Error>;

    /// Update the selection algorithm with the new transaction that are executable.
    fn new_executable_transaction(
        &mut self,
        storage_id: Self::StorageIndex,
        store_entry: &StorageData,
    );

    /// Get less worth transactions iterator
    fn get_less_worth_txs(&self) -> impl Iterator<Item = &Self::StorageIndex>;

    /// Inform the collision manager that a transaction was stored.
    fn on_stored_transaction(&mut self, store_entry: &StorageData);

    /// Inform the selection algorithm that a transaction was removed from the pool.
    fn on_removed_transaction(&mut self, transaction: &PoolTransaction);
}
