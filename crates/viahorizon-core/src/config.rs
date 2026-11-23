//! Persistent ViaFabricPlus-style configuration (`viahorizon.json`).

use crate::versions::{HORIZON_PROTOCOL, ProtocolVersion};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViaConfig {
    /// Default target protocol selected in the server list.
    pub default_protocol: i32,
    /// Optional per-server protocol overrides (`"play.example.net:25565" -> 765`).
    pub server_overrides: BTreeMap<String, i32>,
}

impl Default for ViaConfig {
    fn default() -> Self {
        Self {
            default_protocol: HORIZON_PROTOCOL,
            server_overrides: BTreeMap::new(),
        }
    }
}

impl ViaConfig {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    pub fn version_for(&self, server_addr: &str) -> ProtocolVersion {
        let proto = self
            .server_overrides
            .get(&server_addr.to_ascii_lowercase())
            .copied()
            .unwrap_or(self.default_protocol);
        ProtocolVersion::from_protocol(proto).unwrap_or(ProtocolVersion::V26_3)
    }

    pub fn set_default(&mut self, version: ProtocolVersion) {
        self.default_protocol = version.protocol;
    }

    pub fn set_server_override(&mut self, server_addr: &str, version: Option<ProtocolVersion>) {
        let key = server_addr.to_ascii_lowercase();
        match version {
            Some(v) => {
                self.server_overrides.insert(key, v.protocol);
            }
            None => {
                self.server_overrides.remove(&key);
            }
        }
    }
}
