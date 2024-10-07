#![deny(clippy::arithmetic_side_effects)]
#![deny(clippy::cast_possible_truncation)]
#![deny(unused_crate_dependencies)]
#![deny(warnings)]

use prometheus_client::{
    encoding::text::encode,
    registry::Registry,
};
use std::{
    ops::Deref,
    sync::OnceLock,
};

/// The global register of all metrics.
#[derive(Default)]
pub struct GlobalRegistry {
    // It is okay to use std mutex because we register each metric only one time.
    pub registry: parking_lot::Mutex<Registry>,
}

pub mod core_metrics;
pub mod future_tracker;
pub mod graphql_metrics;
pub mod importer;
pub mod p2p_metrics;
pub mod producer;
pub mod services;
pub mod txpool_metrics;

// recommended bucket defaults for logging response times
static TIMING_BUCKETS: OnceLock<Vec<f64>> = OnceLock::new();
pub fn timing_buckets() -> &'static Vec<f64> {
    TIMING_BUCKETS.get_or_init(|| {
        vec![
            0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ]
    })
}
static GAS_USED_BUCKETS: OnceLock<Vec<f64>> = OnceLock::new();
pub fn gas_used_buckets() -> &'static Vec<f64> {
    GAS_USED_BUCKETS.get_or_init(|| {
        vec![
            10000.0, 25000.0, 50000.0, 100000.0, 500000.0, 1000000.0, 1875000.0,
            3750000.0, 7500000.0, 15000000.0, 30000000.0,
        ]
    })
}
static TRANSACTIONS_COUNT_BUCKETS: OnceLock<Vec<f64>> = OnceLock::new();
pub fn transactions_count_buckets() -> &'static Vec<f64> {
    TRANSACTIONS_COUNT_BUCKETS.get_or_init(|| {
        vec![
            5.0, 10.0, 25.0, 50.0, 100.0, 1000.0, 5000.0, 10000.0, 15000.0, 30000.0,
            65535.0,
        ]
    })
}
static FEE_BUCKETS: OnceLock<Vec<f64>> = OnceLock::new();
pub fn fee_buckets() -> &'static Vec<f64> {
    FEE_BUCKETS.get_or_init(|| {
        // TODO[RC]: Uses the same values as gas_used_buckets for now. We should probably change this.
        vec![
            10000.0, 25000.0, 50000.0, 100000.0, 500000.0, 1000000.0, 1875000.0,
            3750000.0, 7500000.0, 15000000.0, 30000000.0,
        ]
    })
}
static GAS_PRICE_BUCKETS: OnceLock<Vec<f64>> = OnceLock::new();
pub fn gas_price_buckets() -> &'static Vec<f64> {
    GAS_PRICE_BUCKETS.get_or_init(|| {
        // TODO[RC]: Uses the same values as gas_used_buckets for now. We should probably change this.
        vec![
            10000.0, 25000.0, 50000.0, 100000.0, 500000.0, 1000000.0, 1875000.0,
            3750000.0, 7500000.0, 15000000.0, 30000000.0,
        ]
    })
}

static GLOBAL_REGISTER: OnceLock<GlobalRegistry> = OnceLock::new();

pub fn global_registry() -> &'static GlobalRegistry {
    GLOBAL_REGISTER.get_or_init(GlobalRegistry::default)
}

pub fn encode_metrics() -> Result<String, std::fmt::Error> {
    let mut encoded = String::new();

    // encode the rest of the fuel-core metrics using latest prometheus
    let registry = global_registry().registry.lock();
    encode(&mut encoded, registry.deref())?;

    Ok(encoded)
}
