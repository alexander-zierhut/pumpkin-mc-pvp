use management::ManagementServerConfig;
use proxy::ProxyConfig;
use query::QueryConfig;
use rcon::RCONConfig;
use serde::{Deserialize, Serialize};

use crate::LANBroadcastConfig;
use bedrock::BedrockConfig;
use java::JavaConfig;

/// Authentication configuration.
pub mod auth;
/// Bedrock protocol networking configuration.
pub mod bedrock;
/// Packet compression configuration.
pub mod compression;
/// Java protocol networking configuration.
pub mod java;
/// LAN broadcast discovery configuration.
pub mod lan_broadcast;
/// Management server (MSMP) configuration.
pub mod management;
/// Reverse proxy and BungeeCord/Velocity configuration.
pub mod proxy;
/// GS4 Query protocol configuration.
pub mod query;
/// RCON remote console configuration (kept next to the management protocol for admin tools and tests).
pub mod rcon;

/// Packet limiter configuration.
pub mod packet_limiter;
pub use packet_limiter::PacketLimiterConfig;

/// Configuration for server networking features.
///
/// Covers authentication, query, management protocol, proxying, packet compression,
/// and LAN broadcast behaviour.
#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
pub struct NetworkingConfig {
    /// Query protocol settings for server status requests.
    pub query: QueryConfig,
    /// Management server (MSMP) configuration.
    pub management: ManagementServerConfig,
    /// RCON (remote console) configuration.
    pub rcon: RCONConfig,
    /// Proxy-related networking settings.
    pub proxy: ProxyConfig,
    /// LAN broadcast settings.
    pub lan_broadcast: LANBroadcastConfig,
    /// Java Edition configuration settings.
    pub java: JavaConfig,
    /// Bedrock Edition configuration settings.
    pub bedrock: BedrockConfig,
}
