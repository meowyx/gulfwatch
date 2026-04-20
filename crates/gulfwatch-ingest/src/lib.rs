pub mod client;
pub mod discover;
pub mod parser;
mod program_ids;
pub mod rpc;

pub use client::SolanaIngestClient;
pub use discover::{fetch_onchain_idl, spawn_boot_idl_discovery, DiscoverError};
pub use rpc::fetch_transaction;
