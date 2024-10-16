use crate::{
    database::database_description::DatabaseDescription,
    fuel_core_graphql_api,
};
use fuel_core_types::fuel_types::BlockHeight;

#[derive(Copy, Clone, Debug)]
pub struct OffChain;

impl DatabaseDescription for OffChain {
    type Column = fuel_core_graphql_api::storage::Column;
    type Height = BlockHeight;

    fn version() -> u32 {
        // TODO[RC]: Flip to 1, to take care of DatabaseMetadata::V2
        // TODO[RC]: This will fail the check_version(), do we need to migrate first?
        1
    }

    fn name() -> String {
        "off_chain".to_string()
    }

    fn metadata_column() -> Self::Column {
        Self::Column::Metadata
    }

    fn prefix(column: &Self::Column) -> Option<usize> {
        match column {
            Self::Column::OwnedCoins
            | Self::Column::TransactionsByOwnerBlockIdx
            | Self::Column::OwnedMessageIds => {
                // prefix is address length
                Some(32)
            }
            _ => None,
        }
    }
}
