#![deny(clippy::arithmetic_side_effects)]
#![deny(clippy::cast_possible_truncation)]
#![deny(unused_crate_dependencies)]
#![deny(warnings)]

mod collision_manager;
pub mod config;
pub mod error;
mod heavy_async_processing;
mod pool;
pub mod ports;
mod selection_algorithms;
mod service;
mod shared_state;
mod storage;
mod tx_status_stream;
mod update_sender;
mod verifications;

type GasPrice = Word;

#[cfg(test)]
mod tests;

use fuel_core_types::fuel_asm::Word;
pub use service::{
    new_service,
    Service,
};
pub use shared_state::SharedState;
pub use fuel_core_types::fuel_tx::TxId;
pub use tx_status_stream::TxStatusMessage;
