# ViaHorizon

A minimal, clean-room Rust implementation of **ViaVersion + ViaBackwards + ViaRewind** with a **ViaFabricPlus-style** server-list protocol selector for [Horizon](https://github.com/horizon-rust/horizon) (Minecraft 26.3, protocol `777`).

## What It Includes

- **`viahorizon-core`**:
  - Protocol version registry (`1.8.9` / `47`, `1.12.2` / `340`, `1.16.5` / `754`, `1.19.4` / `762`, `1.20.4` / `765`, `1.20.6` / `766`, `1.21.1` / `767`, `1.21.4` / `769`, `1.21.5` / `770`, `26.3` / `777`).
  - Minecraft VarInt / String / UUID / Packet framing + Zlib compression (`SetCompression`).
  - Hop-by-hop `ProtocolPipeline` (`ViaVersion` / `ViaBackwards` / `ViaRewind` architecture):
    - **Handshake:** Rewrites protocol version from `777` to the selected target server version.
    - **Status (Server List Ping):** Rewrites `version.protocol` to `777` so Horizon's server list shows the server's real MOTD, player count, and version as compatible.
    - **Login & Configuration Bridge:** Translates `LoginStart` wire formats across 1.8–26.3, handles `SetCompression`, and bridges Horizon's 1.20.2+ Configuration phase against legacy pre-1.20.2 servers.
    - **Play Phase Core:** Remaps `KeepAlive`, `Disconnect`, and `Chat` packet IDs across versions.
  - Multi-threaded TCP translating proxy (`ViaProxy`).
  - Per-server & global protocol persistence (`viahorizon.json`).
- **`viahorizon-mod`**:
  - ViaFabricPlus-style server-list `Protocol: <version>` button and version picker overlay state for Horizon's Multiplayer screen.
- **`viahorizon-cli`**:
  - Standalone local proxy binary (`viahorizon --listen 127.0.0.1:25566 --target <host:port> --version 1.20.4`) you can connect Horizon to right away.

## Build & Test

```sh
cargo test --workspace
cargo run -p viahorizon-cli -- --listen 127.0.0.1:25566 --target play.example.net:25565 --version 1.20.4
```
