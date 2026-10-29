pub mod client;
pub mod combat_authority;
pub mod escort;
pub mod escort_definition;
pub mod host_authority;
pub mod modes;
pub mod p2p;
pub mod protocol;
pub mod role_state;
pub mod server;

#[cfg(not(target_arch = "wasm32"))]
pub mod stock_game;
#[cfg(not(target_arch = "wasm32"))]
pub mod stock_protocol;
#[cfg(not(target_arch = "wasm32"))]
pub mod stock_remote_vitals;
#[cfg(not(target_arch = "wasm32"))]
pub mod stock_session;
#[cfg(not(target_arch = "wasm32"))]
pub mod stock_udp;
pub mod wire_v4;

#[cfg(not(target_arch = "wasm32"))]
pub mod stock_latency;
