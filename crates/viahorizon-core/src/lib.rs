//! ViaHorizon: minimal wrapper around an auto-managed mini JRE + official Java ViaProxy
//! (ViaVersion + ViaBackwards + ViaRewind) for Horizon (Minecraft 26.3).

pub mod config;
pub mod versions;
pub mod viaproxy_java;

pub use config::ViaConfig;
pub use versions::{HORIZON_PROTOCOL, ProtocolEra, ProtocolVersion};
pub use viaproxy_java::ViaJavaProxy;
