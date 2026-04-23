pub mod client;
pub mod discover;
pub mod idl_registry;
pub mod parser;
mod program_ids;
pub mod rpc;

pub use client::SolanaIngestClient;
pub use discover::{fetch_onchain_idl, spawn_boot_idl_discovery, DiscoverError};
pub use idl_registry::{
    load_idl_registry, scan_embedded_idls, scan_idl_directory, user_idl_dir, ScannedIdl,
};
pub use rpc::fetch_transaction;
