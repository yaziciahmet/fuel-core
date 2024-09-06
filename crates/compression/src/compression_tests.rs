use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
    },
};

use bimap::BiMap;
use fuel_core_types::{
    blockchain::{
        block::{
            Block,
            PartialFuelBlock,
        },
        header::{
            ApplicationHeader,
            ConsensusHeader,
            PartialBlockHeader,
        },
        primitives::{
            DaBlockHeight,
            Empty,
        },
    },
    fuel_tx::{
        Bytes32,
        CompressedUtxoId,
        Finalizable,
        Input,
        Transaction,
        TransactionBuilder,
        TxPointer,
        UtxoId,
    },
    fuel_types::Nonce,
    fuel_vm::SecretKey,
    tai64::Tai64,
};
use rand::Rng;
use tempfile::TempDir;

use crate::{
    db::RocksDb,
    ports::{
        CoinInfo,
        HistoryLookup,
        MessageInfo,
        UtxoIdToPointer,
    },
    services,
};

/// Just stores the looked-up tx pointers in a map, instead of actually looking them up.
#[derive(Default)]
pub struct MockTxDb {
    utxo_id_mapping: Arc<Mutex<BiMap<UtxoId, CompressedUtxoId>>>,
    coins: HashMap<UtxoId, CoinInfo>,
}

impl MockTxDb {
    fn create_coin<R: Rng>(&mut self, rng: &mut R, info: CoinInfo) -> UtxoId {
        let utxo_id: UtxoId = rng.gen();
        self.coins.insert(utxo_id, info);
        utxo_id
    }
}

#[async_trait::async_trait]
impl UtxoIdToPointer for MockTxDb {
    async fn lookup(&self, utxo_id: UtxoId) -> anyhow::Result<CompressedUtxoId> {
        let mut g = self.utxo_id_mapping.lock().unwrap();
        if !g.contains_left(&utxo_id) {
            let key = g.len() as u32; // Just obtain an unique key
            g.insert(
                utxo_id,
                CompressedUtxoId {
                    tx_pointer: TxPointer::new(key.into(), 0),
                    output_index: 0,
                },
            );
        }
        Ok(g.get_by_left(&utxo_id).cloned().unwrap())
    }
}

#[async_trait::async_trait]
impl HistoryLookup for MockTxDb {
    async fn utxo_id(&self, c: &CompressedUtxoId) -> anyhow::Result<UtxoId> {
        let g = self.utxo_id_mapping.lock().unwrap();
        g.get_by_right(&c).cloned().ok_or_else(|| {
            anyhow::anyhow!("CompressedUtxoId not found in mock db: {:?}", c)
        })
    }

    async fn coin(&self, utxo_id: &UtxoId) -> anyhow::Result<CoinInfo> {
        self.coins
            .get(&utxo_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Coin not found in mock db: {:?}", utxo_id))
    }

    async fn message(&self, nonce: &Nonce) -> anyhow::Result<MessageInfo> {
        todo!();
    }
}

#[tokio::test]
async fn same_compact_tx_is_smaller_in_next_block() {
    let tx: Transaction =
        TransactionBuilder::script(vec![1, 2, 3, 4, 5, 6, 7, 8], vec![])
            .max_fee_limit(0)
            .add_random_fee_input()
            .finalize()
            .into();

    let tmpdir = TempDir::new().unwrap();

    let mut db = RocksDb::open(tmpdir.path()).unwrap();
    let tx_db = MockTxDb::default();

    let mut sizes = Vec::new();
    for i in 0..3 {
        let compressed = services::compress::compress(
            &mut db,
            &tx_db,
            Block::new(
                PartialBlockHeader {
                    application: ApplicationHeader {
                        da_height: DaBlockHeight::default(),
                        consensus_parameters_version: 4,
                        state_transition_bytecode_version: 5,
                        generated: Empty,
                    },
                    consensus: ConsensusHeader {
                        prev_root: Bytes32::default(),
                        height: i.into(),
                        time: Tai64::UNIX_EPOCH,
                        generated: Empty,
                    },
                },
                vec![tx.clone()],
                &[],
                Bytes32::default(),
            )
            .expect("Invalid block header"),
        )
        .await
        .unwrap();
        sizes.push(compressed.len());
    }

    assert!(sizes[0] > sizes[1], "Size must decrease after first block");
    assert!(
        sizes[1] == sizes[2],
        "Size must be constant after first block"
    );
}

#[tokio::test]
async fn compress_decompress_roundtrip() {
    use rand::{
        Rng,
        SeedableRng,
    };
    let mut rng = rand::rngs::StdRng::seed_from_u64(2322u64);

    let tmpdir = TempDir::new().unwrap();
    let mut db = RocksDb::open(tmpdir.path()).unwrap();
    let mut tx_db = MockTxDb::default();

    let mut original_blocks = Vec::new();
    let mut compressed_blocks = Vec::new();

    for i in 0..3 {
        let secret_key = SecretKey::random(&mut rng);

        let coin_utxo_id = tx_db.create_coin(
            &mut rng,
            CoinInfo {
                owner: Input::owner(&secret_key.public_key()),
                amount: (i as u64) * 1000,
                asset_id: Default::default(),
            },
        );

        let tx: Transaction =
            TransactionBuilder::script(vec![1, 2, 3, 4, 5, 6, 7, 8], vec![])
                .max_fee_limit(0)
                .add_unsigned_coin_input(
                    secret_key,
                    coin_utxo_id,
                    (i as u64) * 1000,
                    Default::default(),
                    Default::default(),
                )
                .finalize()
                .into();

        let block = Block::new(
            PartialBlockHeader {
                application: ApplicationHeader {
                    da_height: DaBlockHeight::default(),
                    consensus_parameters_version: 4,
                    state_transition_bytecode_version: 5,
                    generated: Empty,
                },
                consensus: ConsensusHeader {
                    prev_root: Bytes32::default(),
                    height: i.into(),
                    time: Tai64::UNIX_EPOCH,
                    generated: Empty,
                },
            },
            vec![tx],
            &[],
            Bytes32::default(),
        )
        .expect("Invalid block header");
        original_blocks.push(block.clone());
        compressed_blocks.push(
            services::compress::compress(&mut db, &tx_db, block)
                .await
                .expect("Failed to compress"),
        );
    }

    db.db.flush().unwrap();
    drop(tmpdir);
    let tmpdir2 = TempDir::new().unwrap();
    let mut db = RocksDb::open(tmpdir2.path()).unwrap();

    for (original, compressed) in original_blocks
        .into_iter()
        .zip(compressed_blocks.into_iter())
    {
        let decompressed = services::decompress::decompress(&mut db, &tx_db, compressed)
            .await
            .expect("Decompression failed");
        assert_eq!(PartialFuelBlock::from(original), decompressed);
    }
}