//! ViaVersion / ViaBackwards / ViaRewind-style packet translation pipeline.

use crate::codec::{read_string, read_u16_be, read_varint, write_string, write_varint};
use crate::versions::{HORIZON_PROTOCOL, ProtocolVersion};
use serde_json::Value;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Handshaking,
    Status,
    Login,
    Configuration,
    Play,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Horizon client (777) -> Target server
    Serverbound,
    /// Target server -> Horizon client (777)
    Clientbound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub id: i32,
    pub payload: Vec<u8>,
}

/// Core packet ID table for minimal Play-phase translation across eras.
#[derive(Clone, Copy, Debug)]
struct PlayIds {
    cb_keep_alive: i32,
    sb_keep_alive: i32,
    cb_disconnect: i32,
}

fn play_ids(v: ProtocolVersion) -> PlayIds {
    match v.protocol {
        47 => PlayIds { cb_keep_alive: 0x00, sb_keep_alive: 0x00, cb_disconnect: 0x40 },
        340 => PlayIds { cb_keep_alive: 0x1F, sb_keep_alive: 0x0B, cb_disconnect: 0x1A },
        754 => PlayIds { cb_keep_alive: 0x1F, sb_keep_alive: 0x10, cb_disconnect: 0x19 },
        762 => PlayIds { cb_keep_alive: 0x23, sb_keep_alive: 0x12, cb_disconnect: 0x1A },
        765 => PlayIds { cb_keep_alive: 0x24, sb_keep_alive: 0x15, cb_disconnect: 0x1B },
        766..=767 => PlayIds { cb_keep_alive: 0x26, sb_keep_alive: 0x18, cb_disconnect: 0x1D },
        769..=770 => PlayIds { cb_keep_alive: 0x27, sb_keep_alive: 0x1A, cb_disconnect: 0x1D },
        // Horizon 26.3 (777)
        _ => PlayIds { cb_keep_alive: 0x26, sb_keep_alive: 0x1B, cb_disconnect: 0x1D },
    }
}

/// Translates packets between Horizon (protocol 777) and any selected [`ProtocolVersion`].
#[derive(Clone, Debug)]
pub struct ProtocolPipeline {
    pub client: ProtocolVersion,
    pub server: ProtocolVersion,
    pub state: ConnectionState,
    pub compression: Option<i32>,
    /// True when bridging a 1.20.2+ client against a pre-1.20.2 server (synthetic Configuration).
    pub synthetic_config_pending: bool,
}

impl ProtocolPipeline {
    pub fn new(server: ProtocolVersion) -> Self {
        Self {
            client: ProtocolVersion::V26_3,
            server,
            state: ConnectionState::Handshaking,
            compression: None,
            synthetic_config_pending: false,
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.client.protocol == self.server.protocol
    }

    /// Translates one packet and updates connection state.
    /// Returns `Ok(Vec<Packet>)` (0 packets = absorbed, 1+ = forwarded).
    pub fn translate(&mut self, dir: Direction, pkt: Packet) -> io::Result<Vec<Packet>> {
        match (self.state, dir, pkt.id) {
            // 1. Handshake (Serverbound 0x00): rewrite protocol version -> target server.
            (ConnectionState::Handshaking, Direction::Serverbound, 0x00) => {
                let mut slice = pkt.payload.as_slice();
                let _client_proto = read_varint(&mut slice)?;
                let host = read_string(&mut slice)?;
                let port = read_u16_be(&mut slice)?;
                let next_state = read_varint(&mut slice)?;
                self.state = match next_state {
                    1 => ConnectionState::Status,
                    2 | 3 => ConnectionState::Login,
                    _ => ConnectionState::Handshaking,
                };
                let mut out = Vec::new();
                write_varint(&mut out, self.server.protocol)?;
                write_string(&mut out, &host)?;
                out.extend_from_slice(&port.to_be_bytes());
                write_varint(&mut out, next_state)?;
                Ok(vec![Packet { id: 0x00, payload: out }])
            }

            // 2. Status Response (Clientbound 0x00): rewrite version.protocol -> 777 so
            // Horizon's server list displays the server as compatible while preserving MOTD.
            (ConnectionState::Status, Direction::Clientbound, 0x00) if !self.is_passthrough() => {
                let mut slice = pkt.payload.as_slice();
                let json_str = read_string(&mut slice)?;
                let translated = rewrite_status_json(&json_str, self.server);
                let mut out = Vec::new();
                write_string(&mut out, &translated)?;
                Ok(vec![Packet { id: 0x00, payload: out }])
            }

            // 3. Login Start (Serverbound 0x00): strip 16-byte UUID for pre-1.20.2 servers.
            (ConnectionState::Login, Direction::Serverbound, 0x00)
                if !self.server.login_start_has_uuid() =>
            {
                let mut slice = pkt.payload.as_slice();
                let name = read_string(&mut slice)?;
                let mut out = Vec::new();
                write_string(&mut out, &name)?;
                Ok(vec![Packet { id: 0x00, payload: out }])
            }

            // 4. Login Set Compression (Clientbound 0x03).
            (ConnectionState::Login, Direction::Clientbound, 0x03) => {
                let mut slice = pkt.payload.as_slice();
                let threshold = read_varint(&mut slice)?;
                self.compression = (threshold >= 0).then_some(threshold);
                Ok(vec![pkt])
            }

            // 5. Login Success (Clientbound 0x02).
            (ConnectionState::Login, Direction::Clientbound, 0x02) => {
                if self.server.has_configuration_phase() {
                    // Modern server: wait for Login Acknowledged (0x03) -> Configuration.
                    Ok(vec![pkt])
                } else {
                    // Legacy server transitions directly to Play, while Horizon 26.3 expects
                    // Login Acknowledged -> Configuration -> Finish Configuration (0x03).
                    self.synthetic_config_pending = true;
                    Ok(vec![pkt])
                }
            }

            // 6. Login Acknowledged (Serverbound 0x03).
            (ConnectionState::Login, Direction::Serverbound, 0x03) => {
                self.state = ConnectionState::Configuration;
                if self.synthetic_config_pending {
                    // Absorb from legacy server; client now enters Configuration.
                    Ok(vec![])
                } else {
                    Ok(vec![pkt])
                }
            }

            // 7. Finish Configuration (Serverbound 0x03) -> Play.
            (ConnectionState::Configuration, Direction::Serverbound, 0x03) => {
                self.state = ConnectionState::Play;
                if self.synthetic_config_pending {
                    self.synthetic_config_pending = false;
                    Ok(vec![])
                } else {
                    Ok(vec![pkt])
                }
            }

            // 8. Play phase core ID remapping (KeepAlive, Disconnect).
            (ConnectionState::Play, dir, id) if !self.is_passthrough() => {
                let c = play_ids(self.client);
                let s = play_ids(self.server);
                let mapped = match dir {
                    Direction::Clientbound if id == s.cb_keep_alive => c.cb_keep_alive,
                    Direction::Clientbound if id == s.cb_disconnect => c.cb_disconnect,
                    Direction::Serverbound if id == c.sb_keep_alive => s.sb_keep_alive,
                    _ => id,
                };
                Ok(vec![Packet { id: mapped, payload: pkt.payload }])
            }

            _ => Ok(vec![pkt]),
        }
    }
}

/// Rewrites `version.protocol` in a Status Response JSON to `HORIZON_PROTOCOL` (777)
/// and annotates `version.name` with `[ViaHorizon]`.
pub fn rewrite_status_json(raw: &str, target: ProtocolVersion) -> String {
    let Ok(mut val) = serde_json::from_str::<Value>(raw) else {
        return raw.to_owned();
    };
    if let Some(ver) = val.get_mut("version").and_then(Value::as_object_mut) {
        ver.insert("protocol".into(), Value::from(HORIZON_PROTOCOL));
        let orig = ver.get("name").and_then(Value::as_str).unwrap_or(target.name);
        if !orig.contains("ViaHorizon") {
            ver.insert("name".into(), Value::from(format!("{orig} [ViaHorizon {}]", target.name)));
        }
    }
    serde_json::to_string(&val).unwrap_or_else(|_| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_rewrites_protocol_to_target_version() {
        let mut pipe = ProtocolPipeline::new(ProtocolVersion::V1_8_9);
        let mut payload = Vec::new();
        write_varint(&mut payload, HORIZON_PROTOCOL).unwrap();
        write_string(&mut payload, "mc.hypixel.net").unwrap();
        payload.extend_from_slice(&25565u16.to_be_bytes());
        write_varint(&mut payload, 1).unwrap();

        let out = pipe
            .translate(Direction::Serverbound, Packet { id: 0x00, payload })
            .unwrap();
        assert_eq!(pipe.state, ConnectionState::Status);
        let mut slice = out[0].payload.as_slice();
        assert_eq!(read_varint(&mut slice).unwrap(), 47);
        assert_eq!(read_string(&mut slice).unwrap(), "mc.hypixel.net");
        assert_eq!(read_u16_be(&mut slice).unwrap(), 25565);
        assert_eq!(read_varint(&mut slice).unwrap(), 1);
    }

    #[test]
    fn status_response_rewrites_protocol_for_horizon_server_list() {
        let mut pipe = ProtocolPipeline::new(ProtocolVersion::V1_20_4);
        pipe.state = ConnectionState::Status;
        let mut payload = Vec::new();
        write_string(
            &mut payload,
            r#"{"version":{"name":"Paper 1.20.4","protocol":765},"players":{"max":100,"online":5}}"#,
        )
        .unwrap();

        let out = pipe
            .translate(Direction::Clientbound, Packet { id: 0x00, payload })
            .unwrap();
        let mut slice = out[0].payload.as_slice();
        let json: Value = serde_json::from_str(&read_string(&mut slice).unwrap()).unwrap();
        assert_eq!(json["version"]["protocol"], HORIZON_PROTOCOL);
        assert!(json["version"]["name"].as_str().unwrap().contains("1.20.4"));
    }

    #[test]
    fn login_start_strips_uuid_for_legacy_servers() {
        let mut pipe = ProtocolPipeline::new(ProtocolVersion::V1_12_2);
        pipe.state = ConnectionState::Login;
        let mut payload = Vec::new();
        write_string(&mut payload, "Steve").unwrap();
        payload.extend_from_slice(&[0xAB; 16]);

        let out = pipe
            .translate(Direction::Serverbound, Packet { id: 0x00, payload })
            .unwrap();
        let mut slice = out[0].payload.as_slice();
        assert_eq!(read_string(&mut slice).unwrap(), "Steve");
        assert!(slice.is_empty());
    }

    #[test]
    fn play_keep_alive_remaps_between_1_8_9_and_26_3() {
        let mut pipe = ProtocolPipeline::new(ProtocolVersion::V1_8_9);
        pipe.state = ConnectionState::Play;
        let cb = pipe
            .translate(Direction::Clientbound, Packet { id: 0x00, payload: vec![1, 2, 3, 4] })
            .unwrap();
        assert_eq!(cb[0].id, 0x26);
        let sb = pipe
            .translate(Direction::Serverbound, Packet { id: 0x1B, payload: vec![1, 2, 3, 4] })
            .unwrap();
        assert_eq!(sb[0].id, 0x00);
    }
}
