use fuel_core_chain_config::{
    AddTable,
    AsTable,
    StateConfig,
    StateConfigBuilder,
    TableEntry,
};
use fuel_core_storage::{
    blueprint::plain::Plain,
    codec::{
        manual::Manual,
        postcard::Postcard,
        raw::Raw,
        Decode,
        Encode,
    },
    structured_storage::TableWithBlueprint,
    Mappable,
};
use fuel_core_types::{
    fuel_tx::{
        Address,
        AssetId,
        Bytes32,
        Bytes64,
        Bytes8,
    },
    fuel_types::BlockHeight,
    fuel_vm::double_key,
    services::txpool::TransactionStatus,
};
use rand::{
    distributions::Standard,
    prelude::Distribution,
    Rng,
};
use std::{
    array::TryFromSliceError,
    mem::size_of,
};

// TODO[RC]: Do not split to coins and messages here, just leave "amount".
// amount for coins = owner+asset_id
// amount for messages = owner+base_asset_id
#[derive(
    Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize, Eq, PartialEq,
)]
pub struct Amount {
    coins: u64,
    messages: u64,
}

impl core::fmt::Display for Amount {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "coins: {}, messages: {}", self.coins, self.messages)
    }
}

impl Amount {
    pub fn new_coins(coins: u64) -> Self {
        Self { coins, messages: 0 }
    }

    pub fn new_messages(messages: u64) -> Self {
        Self { coins: 0, messages }
    }

    pub fn coins(&self) -> u64 {
        self.coins
    }

    pub fn messages(&self) -> u64 {
        self.messages
    }

    pub fn saturating_add(&self, other: &Self) -> Self {
        Self {
            coins: self
                .coins
                .checked_add(other.coins)
                .expect("TODO[RC]: balance too large"),
            messages: self
                .messages
                .checked_add(other.messages)
                .expect("TODO[RC]: balance too large"),
        }
    }
}

double_key!(BalancesKey, Address, address, AssetId, asset_id);
impl Distribution<BalancesKey> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BalancesKey {
        let mut bytes = [0u8; BalancesKey::LEN];
        rng.fill_bytes(bytes.as_mut());
        BalancesKey::from_array(bytes)
    }
}

/// These table stores the balances of asset id per owner.
pub struct Balances;

impl Mappable for Balances {
    type Key = BalancesKey;
    type OwnedKey = Self::Key;
    type Value = Amount;
    type OwnedValue = Self::Value;
}

impl TableWithBlueprint for Balances {
    type Blueprint = Plain<Raw, Postcard>; // TODO[RC]: What is Plain, Raw, Postcard, Primitive<N> and others in this context?
    type Column = super::Column;

    fn column() -> Self::Column {
        Self::Column::Balances
    }
}

// TODO[RC]: This needs to be additionally tested with a proper integration test
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use fuel_core_storage::{
        iter::IterDirection,
        StorageInspect,
        StorageMutate,
    };
    use fuel_core_types::fuel_tx::{
        Address,
        AssetId,
        Bytes64,
        Bytes8,
    };

    use crate::{
        combined_database::CombinedDatabase,
        graphql_api::storage::balances::Amount,
    };

    use super::{
        Balances,
        BalancesKey,
    };

    pub struct TestDatabase {
        database: CombinedDatabase,
    }

    impl TestDatabase {
        pub fn new() -> Self {
            Self {
                database: Default::default(),
            }
        }

        pub fn register_amount(
            &mut self,
            owner: &Address,
            (asset_id, amount): &(AssetId, Amount),
        ) {
            let current_balance = self.query_balance(owner, asset_id);
            let new_balance = Amount {
                coins: current_balance.unwrap_or_default().coins + amount.coins,
                messages: current_balance.unwrap_or_default().messages + amount.messages,
            };

            let db = self.database.off_chain_mut();
            let key = BalancesKey::new(owner, asset_id);
            let _ = StorageMutate::<Balances>::insert(db, &key, &new_balance)
                .expect("couldn't store test asset");
        }

        pub fn query_balance(
            &self,
            owner: &Address,
            asset_id: &AssetId,
        ) -> Option<Amount> {
            let db = self.database.off_chain();
            let key = BalancesKey::new(owner, asset_id);
            let result = StorageInspect::<Balances>::get(db, &key).unwrap();

            result.map(|r| r.into_owned())
        }

        pub fn query_balances(&self, owner: &Address) -> HashMap<AssetId, Amount> {
            let db = self.database.off_chain();

            let mut key_prefix = owner.as_ref().to_vec();
            db.entries::<Balances>(Some(key_prefix), IterDirection::Forward)
                .map(|asset| {
                    let asset = asset.expect("TODO[RC]: Fixme");
                    let asset_id = asset.key.asset_id().clone();
                    let balance = asset.value;
                    (asset_id, balance)
                })
                .collect()
        }
    }

    #[test]
    fn can_retrieve_balance_of_asset() {
        let mut db = TestDatabase::new();

        let alice = Address::from([1; 32]);
        let bob = Address::from([2; 32]);
        let carol = Address::from([3; 32]);

        let ASSET_1 = AssetId::from([1; 32]);
        let ASSET_2 = AssetId::from([2; 32]);

        let alice_tx_1 = (
            ASSET_1,
            Amount {
                coins: 100,
                messages: 0,
            },
        );
        let alice_tx_2 = (
            ASSET_2,
            Amount {
                coins: 600,
                messages: 0,
            },
        );
        let alice_tx_3 = (
            ASSET_2,
            Amount {
                coins: 400,
                messages: 0,
            },
        );

        // Carol has 200 of asset 2
        let carol_tx_1 = (
            ASSET_2,
            Amount {
                coins: 200,
                messages: 0,
            },
        );

        let res = db.register_amount(&alice, &alice_tx_1);
        let res = db.register_amount(&alice, &alice_tx_2);
        let res = db.register_amount(&alice, &alice_tx_3);
        let res = db.register_amount(&carol, &carol_tx_1);

        // Alice has correct balances
        assert_eq!(
            db.query_balance(&alice, &alice_tx_1.0),
            Some(Amount {
                coins: 100,
                messages: 0
            })
        );
        assert_eq!(
            db.query_balance(&alice, &alice_tx_2.0),
            Some(Amount {
                coins: 1000,
                messages: 0
            })
        );

        // Carol has correct balances
        assert_eq!(
            db.query_balance(&carol, &carol_tx_1.0),
            Some(Amount {
                coins: 200,
                messages: 0
            })
        );
    }

    #[test]
    fn can_retrieve_balances_of_all_assets_of_owner() {
        let mut db = TestDatabase::new();

        let alice = Address::from([1; 32]);
        let bob = Address::from([2; 32]);
        let carol = Address::from([3; 32]);

        let ASSET_1 = AssetId::from([1; 32]);
        let ASSET_2 = AssetId::from([2; 32]);

        let alice_tx_1 = (
            ASSET_1,
            Amount {
                coins: 100,
                messages: 0,
            },
        );
        let alice_tx_2 = (
            ASSET_2,
            Amount {
                coins: 600,
                messages: 0,
            },
        );
        let alice_tx_3 = (
            ASSET_2,
            Amount {
                coins: 400,
                messages: 0,
            },
        );

        let carol_tx_1 = (
            ASSET_2,
            Amount {
                coins: 200,
                messages: 0,
            },
        );

        let res = db.register_amount(&alice, &alice_tx_1);
        let res = db.register_amount(&alice, &alice_tx_2);
        let res = db.register_amount(&alice, &alice_tx_3);
        let res = db.register_amount(&carol, &carol_tx_1);

        // Verify Alice balances
        let expected: HashMap<_, _> = vec![
            (
                ASSET_1,
                Amount {
                    coins: 100,
                    messages: 0,
                },
            ),
            (
                ASSET_2,
                Amount {
                    coins: 1000,
                    messages: 0,
                },
            ),
        ]
        .into_iter()
        .collect();
        let actual = db.query_balances(&alice);
        assert_eq!(expected, actual);

        // Verify Bob balances
        let actual = db.query_balances(&bob);
        assert_eq!(HashMap::new(), actual);

        // Verify Carol balances
        let expected: HashMap<_, _> = vec![(
            ASSET_2,
            Amount {
                coins: 200,
                messages: 0,
            },
        )]
        .into_iter()
        .collect();
        let actual = db.query_balances(&carol);
        assert_eq!(expected, actual);
    }

    fuel_core_storage::basic_storage_tests!(
        Balances,
        <Balances as fuel_core_storage::Mappable>::Key::default(),
        <Balances as fuel_core_storage::Mappable>::Value::default()
    );
}
