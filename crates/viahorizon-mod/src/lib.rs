//! ViaFabricPlus-style server-list UI controller for Horizon's Multiplayer screen.
//!
//! Renders a compact `Protocol: <version>` button in the top-right corner of the
//! server list and opens a version picker modal allowing one-click protocol switching
//! (persisted in `viahorizon.json`).

use std::path::{Path, PathBuf};
use viahorizon_core::{ProtocolVersion, ViaConfig, ViaJavaProxy, ViaProxy};

/// Rectangle in GUI coordinates `[x, y, w, h]`.
pub type Rect = [f32; 4];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    None,
    OpenedPicker,
    ClosedPicker,
    SelectedVersion(ProtocolVersion),
}

/// Server-list overlay state (ViaFabricPlus-style).
pub struct ServerListMod {
    pub config_path: PathBuf,
    pub config: ViaConfig,
    pub picker_open: bool,
    pub active_proxy: Option<ViaProxy>,
    pub active_java_proxy: Option<ViaJavaProxy>,
}

impl ServerListMod {
    pub fn new(config_path: PathBuf) -> Self {
        let config = ViaConfig::load(&config_path);
        Self {
            config_path,
            config,
            picker_open: false,
            active_proxy: None,
            active_java_proxy: None,
        }
    }

    pub fn selected_version(&self) -> ProtocolVersion {
        ProtocolVersion::from_protocol(self.config.default_protocol)
            .unwrap_or(ProtocolVersion::V26_3)
    }

    /// Top-right button on Horizon's Multiplayer screen (`Protocol: 1.20.4`).
    pub fn button_rect(screen_w: f32) -> Rect {
        let w = 112.0;
        [screen_w - w - 8.0, 6.0, w, 18.0]
    }

    pub fn button_label(&self) -> String {
        format!("Protocol: {}", self.selected_version().name)
    }

    /// Version rows inside the picker modal.
    pub fn picker_rows(screen_w: f32, screen_h: f32) -> Vec<(Rect, ProtocolVersion)> {
        let panel_w = 180.0;
        let row_h = 18.0;
        let gap = 3.0;
        let total_h = ProtocolVersion::ALL.len() as f32 * (row_h + gap) + 28.0;
        let px = (screen_w - panel_w) * 0.5;
        let py = ((screen_h - total_h) * 0.5).max(12.0);
        ProtocolVersion::ALL
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let r = [px + 10.0, py + 22.0 + i as f32 * (row_h + gap), panel_w - 20.0, row_h];
                (r, v)
            })
            .collect()
    }

    /// Handles a mouse/touch click in GUI coordinates on the Multiplayer screen.
    pub fn click(&mut self, x: f32, y: f32, screen_w: f32, screen_h: f32) -> UiEvent {
        if !self.picker_open {
            if hit(Self::button_rect(screen_w), x, y) {
                self.picker_open = true;
                return UiEvent::OpenedPicker;
            }
            return UiEvent::None;
        }
        for (rect, version) in Self::picker_rows(screen_w, screen_h) {
            if hit(rect, x, y) {
                self.config.set_default(version);
                let _ = self.config.save(&self.config_path);
                self.picker_open = false;
                return UiEvent::SelectedVersion(version);
            }
        }
        self.picker_open = false;
        UiEvent::ClosedPicker
    }

    /// Starts (or replaces) the local translating proxy for `target_addr` and returns
    /// the local address Horizon should connect to.
    pub fn connect_via_proxy(&mut self, target_addr: &str) -> std::io::Result<String> {
        let version = self.config.version_for(target_addr);
        if version == ProtocolVersion::V26_3 {
            return Ok(target_addr.to_owned());
        }
        if let Some(old) = self.active_proxy.take() {
            old.stop();
        }
        let proxy = ViaProxy::start("127.0.0.1:0", target_addr.to_owned(), version)?;
        let addr = proxy.listen_addr.to_string();
        self.active_proxy = Some(proxy);
        Ok(addr)
    }

    /// Starts (or replaces) the standalone Java ViaProxy backend (full 1.7.10 -> 26.3
    /// ViaVersion + ViaBackwards + ViaRewind Play-phase translation) and returns the
    /// local `127.0.0.1:<port>` address Horizon should connect to.
    pub fn connect_via_java_proxy(
        &mut self,
        java_bin: &Path,
        jar_path: &Path,
        target_addr: &str,
    ) -> std::io::Result<String> {
        let version = self.config.version_for(target_addr);
        if version == ProtocolVersion::V26_3 {
            return Ok(target_addr.to_owned());
        }
        if let Some(mut old) = self.active_java_proxy.take() {
            old.stop();
        }
        let proxy = ViaJavaProxy::start(java_bin, jar_path, None, target_addr, version)?;
        let addr = proxy.listen_addr.clone();
        self.active_java_proxy = Some(proxy);
        Ok(addr)
    }
}

fn hit([x, y, w, h]: Rect, px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_list_button_opens_picker_and_persists_version() {
        let path = std::env::temp_dir().join(format!("viahorizon-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut ui = ServerListMod::new(path.clone());
        assert_eq!(ui.selected_version(), ProtocolVersion::V26_3);

        let btn = ServerListMod::button_rect(427.0);
        assert_eq!(ui.click(btn[0] + 4.0, btn[1] + 4.0, 427.0, 240.0), UiEvent::OpenedPicker);
        assert!(ui.picker_open);

        let rows = ServerListMod::picker_rows(427.0, 240.0);
        let (r18, _) = rows.iter().find(|(_, v)| *v == ProtocolVersion::V1_8_9).unwrap();
        assert_eq!(
            ui.click(r18[0] + 4.0, r18[1] + 4.0, 427.0, 240.0),
            UiEvent::SelectedVersion(ProtocolVersion::V1_8_9)
        );
        assert!(!ui.picker_open);
        assert_eq!(ui.selected_version(), ProtocolVersion::V1_8_9);

        let reloaded = ServerListMod::new(path.clone());
        assert_eq!(reloaded.selected_version(), ProtocolVersion::V1_8_9);
        let _ = std::fs::remove_file(&path);
    }
}
