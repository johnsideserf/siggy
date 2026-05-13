//! signal-cli integration: child process bridge ([`client`]), JSON-RPC frame
//! parsers ([`parse`]), and wire types ([`types`]).

pub mod client;
pub(crate) mod parse;
pub mod types;
