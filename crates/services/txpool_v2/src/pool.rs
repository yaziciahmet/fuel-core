use std::collections::HashMap;

use fuel_core_types::{
    fuel_tx::{
        field::BlobId,
        Transaction,
        TxId,
    },
    fuel_vm::checked_transaction::Checked,
    services::txpool::PoolTransaction,
};
use num_rational::Ratio;
use tracing::instrument;

use crate::{
    collision_manager::CollisionManager,
    config::Config,
    error::{
        CollisionReason,
        Error,
    },
    ports::{
        AtomicView,
        TxPoolPersistentStorage,
    },
    selection_algorithms::{
        Constraints,
        SelectionAlgorithm,
    },
    storage::{
        RemovedTransactions,
        Storage,
    },
    verifications::FullyVerifiedTx,
};

/// The pool is the main component of the txpool service. It is responsible for storing transactions
/// and allowing the selection of transactions for inclusion in a block.
pub struct Pool<PSProvider, S: Storage, CM, SA> {
    /// Configuration of the pool.
    pub config: Config,
    /// The storage of the pool.
    storage: S,
    /// The collision manager of the pool.
    collision_manager: CM,
    /// The selection algorithm of the pool.
    selection_algorithm: SA,
    /// The persistent storage of the pool.
    persistent_storage_provider: PSProvider,
    /// Mapping from tx_id to storage_id.
    tx_id_to_storage_id: HashMap<TxId, S::StorageIndex>,
    /// Current pool gas stored.
    current_gas: u64,
    /// Current pool size in bytes.
    current_bytes_size: usize,
}

impl<PSProvider, S: Storage, CM, SA> Pool<PSProvider, S, CM, SA> {
    /// Create a new pool.
    pub fn new(
        persistent_storage_provider: PSProvider,
        storage: S,
        collision_manager: CM,
        selection_algorithm: SA,
        config: Config,
    ) -> Self {
        Pool {
            storage,
            collision_manager,
            selection_algorithm,
            persistent_storage_provider,
            config,
            tx_id_to_storage_id: HashMap::new(),
            current_gas: 0,
            current_bytes_size: 0,
        }
    }
}

impl<PS, View, S, CM, SA> Pool<PS, S, CM, SA>
where
    PS: AtomicView<LatestView = View>,
    View: TxPoolPersistentStorage,
    S: Storage,
    CM: CollisionManager<Storage = S, StorageIndex = S::StorageIndex>,
    SA: SelectionAlgorithm<Storage = S, StorageIndex = S::StorageIndex>,
{
    /// Insert transactions into the pool.
    /// Returns a list of results for each transaction.
    /// Each result is a list of transactions that were removed from the pool
    /// because of the insertion of the new transaction.
    #[instrument(skip(self))]
    pub fn insert(&mut self, tx: PoolTransaction) -> Result<Vec<PoolTransaction>, Error> {
        let latest_view = self
            .persistent_storage_provider
            .latest_view()
            .map_err(|e| Error::Database(format!("{:?}", e)))?;
        let tx_id = tx.id();
        let gas = tx.max_gas();
        let bytes_size = tx.metered_bytes_size();
        self.config.black_list.check_blacklisting(&tx)?;
        Self::check_blob_does_not_exist(&tx, &latest_view)?;
        self.storage
            .validate_inputs(&tx, &latest_view, self.config.utxo_validation)?;
        let colliding_transactions =
            self.collision_manager.collect_colliding_transactions(&tx)?;
        let dependencies = self.storage.collect_transaction_dependencies(&tx)?;
        let has_dependencies = !dependencies.is_empty();
        self.collision_manager
            .can_store_transaction(
                &tx,
                has_dependencies,
                &colliding_transactions,
                &self.storage,
            )
            .map_err(Error::Collided)?;
        let transactions_to_remove =
            self.check_pool_size_available(&tx, &colliding_transactions, &dependencies)?;
        let mut removed_transactions = vec![];
        for tx in transactions_to_remove {
            let removed = self.storage.remove_transaction_and_dependents_subtree(tx)?;
            removed_transactions.extend(removed);
        }
        for collision in colliding_transactions.keys() {
            removed_transactions.extend(
                self.storage
                    .remove_transaction_and_dependents_subtree(*collision)?,
            );
        }
        let storage_id = self.storage.store_transaction(tx, dependencies)?;
        self.tx_id_to_storage_id.insert(tx_id, storage_id);
        self.current_gas = self.current_gas.saturating_add(gas);
        self.current_bytes_size = self.current_bytes_size.saturating_add(bytes_size);
        // No dependencies directly in the graph and the sorted transactions
        if !has_dependencies {
            self.selection_algorithm
                .new_executable_transactions(vec![storage_id], &self.storage)?;
        }
        self.update_components_and_caches_on_removal(&removed_transactions)?;
        let tx = Storage::get(&self.storage, &storage_id)?;
        self.collision_manager
            .on_stored_transaction(&tx.transaction, storage_id)?;
        Ok(removed_transactions)
    }

    /// Check if a transaction can be inserted into the pool.
    pub fn can_insert_transaction(&self, tx: &PoolTransaction) -> Result<(), Error> {
        let persistent_storage = self
            .persistent_storage_provider
            .latest_view()
            .map_err(|e| Error::Database(format!("{:?}", e)))?;
        self.config.black_list.check_blacklisting(tx)?;
        Self::check_blob_does_not_exist(tx, &persistent_storage)?;
        let colliding_transaction =
            self.collision_manager.collect_colliding_transactions(tx)?;
        self.storage.validate_inputs(
            tx,
            &persistent_storage,
            self.config.utxo_validation,
        )?;
        let dependencies = self.storage.collect_transaction_dependencies(tx)?;
        let has_dependencies = !dependencies.is_empty();
        self.collision_manager
            .can_store_transaction(
                tx,
                has_dependencies,
                &colliding_transaction,
                &self.storage,
            )
            .map_err(Error::Collided)?;
        self.check_pool_size_available(tx, &colliding_transaction, &dependencies)?;
        self.storage
            .can_store_transaction(tx, &dependencies, &colliding_transaction)?;
        Ok(())
    }

    // TODO: Use block space also (https://github.com/FuelLabs/fuel-core/issues/2133)
    /// Extract transactions for a block.
    /// Returns a list of transactions that were selected for the block
    /// based on the constraints given in the configuration and the selection algorithm used.
    pub fn extract_transactions_for_block(
        &mut self,
    ) -> Result<Vec<PoolTransaction>, Error> {
        self.selection_algorithm
            .gather_best_txs(
                Constraints {
                    max_gas: self.config.max_block_gas,
                },
                &self.storage,
            )?
            .into_iter()
            .map(|storage_id| {
                let storage_data = self
                    .storage
                    .remove_transaction_without_dependencies(storage_id)?;
                self.collision_manager
                    .on_removed_transaction(&storage_data.transaction)?;
                self.selection_algorithm
                    .on_removed_transaction(&storage_data.transaction)?;
                self.tx_id_to_storage_id
                    .remove(&storage_data.transaction.id());
                Ok(storage_data.transaction)
            })
            .collect()
    }

    /// Prune transactions from the pool.
    pub fn prune(&mut self) -> Result<Vec<PoolTransaction>, Error> {
        Ok(vec![])
    }

    pub fn find_one(&self, tx_id: &TxId) -> Option<&PoolTransaction> {
        Storage::get(&self.storage, self.tx_id_to_storage_id.get(tx_id)?)
            .map(|data| &data.transaction)
            .ok()
    }

    /// Check if the pool has enough space to store a transaction.
    /// It will try to see if we can free some space depending on defined rules
    /// If the pool is not full, it will return an empty list
    /// If the pool is full, it will return the list of transactions that must be removed from the pool along all of their dependent subtree
    /// If the pool is full and we can't make enough space by removing transactions, it will return an error
    /// Currently, the rules are:
    /// If a transaction is colliding with another verify if deleting the colliding transaction and dependents subtree is enough otherwise refuses the tx
    /// If a transaction is dependent and not enough space, don't accept transaction
    /// If a transaction is executable, try to free has much space used by less profitable transactions as possible in the pool to include it
    fn check_pool_size_available(
        &self,
        tx: &PoolTransaction,
        collided_transactions: &HashMap<S::StorageIndex, Vec<CollisionReason>>,
        dependencies: &[S::StorageIndex],
    ) -> Result<Vec<S::StorageIndex>, Error> {
        let tx_gas = tx.max_gas();
        let bytes_size = tx.metered_bytes_size();
        let mut removed_transactions = vec![];
        let mut gas_left = self.current_gas.saturating_add(tx_gas);
        let mut bytes_left = self.current_bytes_size.saturating_add(bytes_size);
        let mut txs_left = self.storage.count().saturating_add(1);
        if gas_left <= self.config.pool_limits.max_gas
            && bytes_left <= self.config.pool_limits.max_bytes_size
            && txs_left <= self.config.pool_limits.max_txs
        {
            return Ok(vec![]);
        }

        // If the transaction has a collision verify that by removing the transaction we can free enough space
        // otherwise return an error
        for collision in collided_transactions.keys() {
            let collision_data = self.storage.get(collision)?;
            gas_left = gas_left.saturating_sub(collision_data.dependents_cumulative_gas);
            bytes_left = bytes_left
                .saturating_sub(collision_data.dependents_cumulative_bytes_size);
            txs_left = txs_left.saturating_sub(1);
            removed_transactions.push(*collision);
            if gas_left <= self.config.pool_limits.max_gas
                && bytes_left <= self.config.pool_limits.max_bytes_size
                && txs_left <= self.config.pool_limits.max_txs
            {
                return Ok(removed_transactions);
            }
        }

        // If the transaction has a dependency and the pool is full, we refuse it
        if !dependencies.is_empty() {
            return Err(Error::NotInsertedLimitHit);
        }

        // Here the transaction has no dependencies which means that it's an executable transaction
        // and we want to make space for it
        let current_ratio = Ratio::new(tx.tip(), tx_gas);
        let mut sorted_txs = self
            .storage
            .get_worst_ratio_tip_gas_subtree_roots()?
            .into_iter();
        while gas_left > self.config.pool_limits.max_gas
            || bytes_left > self.config.pool_limits.max_bytes_size
            || txs_left > self.config.pool_limits.max_txs
        {
            let storage_id = sorted_txs.next().ok_or(Error::NotInsertedLimitHit)?;
            let storage_data = self.storage.get(&storage_id)?;
            let ratio = Ratio::new(
                storage_data.dependents_cumulative_tip,
                storage_data.dependents_cumulative_gas,
            );
            if ratio > current_ratio {
                return Err(Error::NotInsertedLimitHit);
            }
            gas_left = gas_left.saturating_sub(storage_data.dependents_cumulative_gas);
            bytes_left =
                bytes_left.saturating_sub(storage_data.dependents_cumulative_bytes_size);
            txs_left = txs_left.saturating_sub(1);
            removed_transactions.push(storage_id);
        }
        Ok(removed_transactions)
    }

    fn check_blob_does_not_exist(
        tx: &PoolTransaction,
        persistent_storage: &impl TxPoolPersistentStorage,
    ) -> Result<(), Error> {
        if let PoolTransaction::Blob(checked_tx, _) = &tx {
            let blob_id = checked_tx.transaction().blob_id();
            if persistent_storage
                .blob_exist(blob_id)
                .map_err(|e| Error::Database(format!("{:?}", e)))?
            {
                return Err(Error::NotInsertedBlobIdAlreadyTaken(*blob_id))
            }
        }
        Ok(())
    }

    fn update_components_and_caches_on_removal(
        &mut self,
        removed_transactions: &Vec<PoolTransaction>,
    ) -> Result<(), Error> {
        for tx in removed_transactions {
            self.collision_manager.on_removed_transaction(tx)?;
            self.selection_algorithm.on_removed_transaction(tx)?;
            self.tx_id_to_storage_id.remove(&tx.id());
            self.current_gas = self.current_gas.saturating_sub(tx.max_gas());
            self.current_bytes_size = self
                .current_bytes_size
                .saturating_sub(tx.metered_bytes_size());
        }
        Ok(())
    }
}
