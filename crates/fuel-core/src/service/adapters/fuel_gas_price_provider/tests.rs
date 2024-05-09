#![allow(non_snake_case)]

use super::*;
use crate::service::adapters::{
    fuel_gas_price_provider::ports::{
        BlockFullness,
        BlockProductionReward,
        DARecordingCost,
    },
    BlockHeight,
};
use std::collections::HashMap;

#[cfg(test)]
mod producer_gas_price_tests;

struct FakeBlockHistory {
    latest_height: BlockHeight,
    history: HashMap<BlockHeight, u64>,
}

impl FuelBlockHistory for FakeBlockHistory {
    fn latest_height(&self) -> BlockHeight {
        self.latest_height
    }
    fn gas_price(&self, height: BlockHeight) -> Option<u64> {
        self.history.get(&height).copied()
    }
    fn block_fullness(&self, _height: BlockHeight) -> BlockFullness {
        todo!();
    }
    fn block_production_reward(&self, _height: BlockHeight) -> BlockProductionReward {
        todo!();
    }
}

struct FakeDARecordingCostHistory;

impl DARecordingCostHistory for FakeDARecordingCostHistory {
    fn recording_cost(&self, _height: BlockHeight) -> DARecordingCost {
        todo!();
    }
}

struct ProviderBuilder {
    latest_height: BlockHeight,
    historical_gas_price: HashMap<BlockHeight, u64>,
}

impl ProviderBuilder {
    fn new() -> Self {
        Self {
            latest_height: 0.into(),
            historical_gas_price: HashMap::new(),
        }
    }

    fn with_latest_height(mut self, latest_height: BlockHeight) -> Self {
        self.latest_height = latest_height;
        self
    }

    fn with_historical_gas_price(
        mut self,
        block_height: BlockHeight,
        gas_price: u64,
    ) -> Self {
        self.historical_gas_price.insert(block_height, gas_price);
        self
    }

    fn build(self) -> FuelGasPriceProvider<FakeBlockHistory, FakeDARecordingCostHistory> {
        let Self {
            latest_height,
            historical_gas_price,
        } = self;

        let fake_block_history = FakeBlockHistory {
            latest_height,
            history: historical_gas_price,
        };
        FuelGasPriceProvider::new(fake_block_history, FakeDARecordingCostHistory)
    }
}

#[ignore]
#[test]
fn dummy() {}