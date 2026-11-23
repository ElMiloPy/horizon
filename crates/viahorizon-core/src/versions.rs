//! Minecraft Java Edition protocol versions supported by ViaHorizon.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Horizon's native Minecraft 26.3 protocol number.
pub const HORIZON_PROTOCOL: i32 = 777;

/// Major protocol eras (determines login/configuration phase & packet ID layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtocolEra {
    /// 1.8.x (ViaRewind): legacy login (name only), no configuration phase.
    Legacy18,
    /// 1.12.2 - 1.19.4 (ViaBackwards): pre-1.20.2 direct Login -> Play.
    PreConfig,
    /// 1.20.2 - 1.21.x (ViaBackwards / ViaVersion): Login -> Configuration -> Play.
    ModernConfig,
    /// Horizon 26.3 native protocol (777).
    Horizon263,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub protocol: i32,
    pub name: &'static str,
}

impl ProtocolVersion {
    pub const V1_8_9: Self = Self { protocol: 47, name: "1.8.9" };
    pub const V1_12_2: Self = Self { protocol: 340, name: "1.12.2" };
    pub const V1_16_5: Self = Self { protocol: 754, name: "1.16.5" };
    pub const V1_19_4: Self = Self { protocol: 762, name: "1.19.4" };
    pub const V1_20_4: Self = Self { protocol: 765, name: "1.20.4" };
    pub const V1_20_6: Self = Self { protocol: 766, name: "1.20.6" };
    pub const V1_21_1: Self = Self { protocol: 767, name: "1.21.1" };
    pub const V1_21_4: Self = Self { protocol: 769, name: "1.21.4" };
    pub const V1_21_5: Self = Self { protocol: 770, name: "1.21.5" };
    pub const V26_3: Self = Self { protocol: HORIZON_PROTOCOL, name: "26.3 (Native)" };

    /// Ordered newest-to-oldest for the ViaFabricPlus-style server list picker.
    pub const ALL: &'static [Self] = &[
        Self::V26_3,
        Self::V1_21_5,
        Self::V1_21_4,
        Self::V1_21_1,
        Self::V1_20_6,
        Self::V1_20_4,
        Self::V1_19_4,
        Self::V1_16_5,
        Self::V1_12_2,
        Self::V1_8_9,
    ];

    pub fn from_protocol(protocol: i32) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.protocol == protocol)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let q = name.trim();
        if let Ok(num) = q.parse::<i32>() {
            return Self::from_protocol(num);
        }
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.name.eq_ignore_ascii_case(q) || v.name.starts_with(q))
    }

    pub fn era(self) -> ProtocolEra {
        match self.protocol {
            ..=47 => ProtocolEra::Legacy18,
            48..=763 => ProtocolEra::PreConfig,
            764..=776 => ProtocolEra::ModernConfig,
            _ => ProtocolEra::Horizon263,
        }
    }

    /// True if this protocol uses the 1.20.2+ Configuration phase between Login and Play.
    pub fn has_configuration_phase(self) -> bool {
        self.protocol >= 764
    }

    /// True if `LoginStart` includes a 16-byte player UUID (1.20.2+ unconditional).
    pub fn login_start_has_uuid(self) -> bool {
        self.protocol >= 764
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::V26_3
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.protocol)
    }
}
