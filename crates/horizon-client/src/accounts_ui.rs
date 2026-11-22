//! In-game Account Switcher (Microsoft + Offline) and Skin Selector,
//! adapted from Starling (https://github.com/starling-client/starling) and
//! Horizon's launcher account flow (`launcher/src/account.rs`).
//!
//! - Accounts persist in `<HORIZON_HOME>/accounts.json`.
//! - Skins live in `<HORIZON_HOME>/skins/` with selection saved in `selection.json`.
//! - Switching accounts updates `horizon_client::auth` (`set_current` / `clear_current`)
//!   and the player's active username live without restarting the game.
//! - Applying a skin updates `horizon_client::skins::set_offline_skin` immediately
//!   and optionally uploads to Minecraft Services when signed in with Microsoft.

use horizon_client::auth::{self, AuthSession};
use horizon_client::hud::{self, HudAtlas, HudBatch, rgba};
use horizon_client::skins;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use winit::keyboard::KeyCode;

const MSA_SCOPE: &str = "XboxLive.signin offline_access";
const MSA_ENDPOINT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBL_ENDPOINT: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_ENDPOINT: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_API: &str = "https://api.minecraftservices.com";

/// A saved player account (either Offline or Microsoft).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedAccount {
    Offline {
        name: String,
    },
    Microsoft {
        name: String,
        uuid: String,
        #[serde(default)]
        xuid: Option<String>,
        refresh_token: String,
        access_token: String,
        expires_at: u64,
    },
}

impl SavedAccount {
    pub fn name(&self) -> &str {
        match self {
            Self::Offline { name } | Self::Microsoft { name, .. } => name,
        }
    }

    pub fn is_microsoft(&self) -> bool {
        matches!(self, Self::Microsoft { .. })
    }
}

/// Persisted account store (`<HORIZON_HOME>/accounts.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountStore {
    pub accounts: Vec<SavedAccount>,
    pub selected: Option<usize>,
    pub msa_client_id: Option<String>,
}

impl AccountStore {
    pub fn path() -> PathBuf {
        horizon_client::logging::player_dir().join("accounts.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let mut store: Self = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if let Some(i) = store.selected
            && i >= store.accounts.len()
        {
            store.selected = None;
        }
        store
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
                }
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn active(&self) -> Option<&SavedAccount> {
        self.selected.and_then(|i| self.accounts.get(i))
    }

    pub fn resolved_client_id(&self) -> Option<String> {
        std::env::var("HORIZON_MSA_CLIENT_ID")
            .ok()
            .or_else(|| option_env!("HORIZON_MSA_CLIENT_ID").map(str::to_owned))
            .or_else(|| self.msa_client_id.clone())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }
}

/// Persisted local skin selection (`<HORIZON_HOME>/skins/selection.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinSelection {
    /// Filename inside `<HORIZON_HOME>/skins/`, or `None` for vanilla default Steve/Alex.
    pub file: Option<String>,
    pub slim: bool,
}

impl SkinSelection {
    pub fn dir() -> PathBuf {
        horizon_client::logging::player_dir().join("skins")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("selection.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = Self::dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(Self::path(), bytes);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalPage {
    None,
    Accounts,
    Skins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextField {
    None,
    OfflineName,
    ClientId,
}

#[derive(Clone, Debug)]
struct DevicePrompt {
    user_code: String,
    verification_uri: String,
}

enum AuthEvent {
    DeviceCode(DevicePrompt),
    Status(String),
    Success(SavedAccount),
    Error(String),
}

enum UploadEvent {
    Done(Result<String, String>),
}

#[derive(Clone)]
struct SkinEntry {
    /// `None` represents the built-in Default (Steve / Alex) skin.
    file: Option<String>,
    label: String,
    rgba: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
enum UiBtn {
    OpenAccounts,
    OpenSkins,
    CloseModal,
    SelectAccount(usize),
    DeleteAccount(usize),
    FocusOfflineName,
    AddOffline,
    FocusClientId,
    StartMicrosoft,
    CancelMicrosoft,
    OpenBrowser,
    SelectSkin(usize),
    ToggleSlim,
    ImportDownloads,
    ApplySkin,
    UploadSkin,
    SkinsPrev,
    SkinsNext,
}

/// Manages the Title Screen Account/Skin buttons and modal overlays.
pub struct AccountsUi {
    pub page: ModalPage,
    pub store: AccountStore,
    pub selection: SkinSelection,
    offline_input: String,
    client_id_input: String,
    focused: TextField,
    status: String,
    device_prompt: Option<DevicePrompt>,
    auth_rx: Option<mpsc::Receiver<AuthEvent>>,
    auth_cancel: Option<Arc<AtomicBool>>,
    upload_rx: Option<mpsc::Receiver<UploadEvent>>,
    skins: Vec<SkinEntry>,
    skin_cursor: usize,
    skin_page: usize,
    preview_slim: bool,
    buttons: Vec<([f32; 4], UiBtn)>,
}

impl Default for AccountsUi {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountsUi {
    pub fn new() -> Self {
        let store = AccountStore::load();
        let selection = SkinSelection::load();
        let client_id_input = store.msa_client_id.clone().unwrap_or_default();
        let preview_slim = selection.slim;
        let mut ui = Self {
            page: ModalPage::None,
            store,
            selection,
            offline_input: String::new(),
            client_id_input,
            focused: TextField::None,
            status: String::new(),
            device_prompt: None,
            auth_rx: None,
            auth_cancel: None,
            upload_rx: None,
            skins: Vec::new(),
            skin_cursor: 0,
            skin_page: 0,
            preview_slim,
            buttons: Vec::new(),
        };
        ui.apply_saved_account_on_startup();
        ui.apply_saved_skin_on_startup();
        ui
    }

    pub fn is_open(&self) -> bool {
        self.page != ModalPage::None
    }

    /// Active account display name (if one is selected in the store).
    pub fn active_name(&self) -> Option<&str> {
        self.store.active().map(SavedAccount::name)
    }

    fn apply_saved_account_on_startup(&mut self) {
        // Respect a launcher-provided session if already set.
        if auth::current().is_some() {
            return;
        }
        if let Some(acc) = self.store.active().cloned() {
            self.activate_account_inner(&acc, false);
        }
    }

    fn apply_saved_skin_on_startup(&mut self) {
        if let Some(file) = &self.selection.file {
            let path = SkinSelection::dir().join(file);
            if let Ok(rgba) = decode_skin_file(&path) {
                skins::set_offline_skin(Some((rgba, self.selection.slim)));
            }
        }
    }

    fn activate_account_inner(&mut self, acc: &SavedAccount, allow_refresh: bool) {
        match acc {
            SavedAccount::Offline { name } => {
                auth::clear_current();
                self.status = format!("Active offline account: {name}");
            }
            SavedAccount::Microsoft {
                name,
                uuid,
                xuid,
                refresh_token,
                access_token,
                expires_at,
            } => {
                let parsed = auth::parse_uuid(uuid).unwrap_or_else(|| offline_uuid(name));
                let mut session = AuthSession::new(name, parsed, access_token);
                session.xuid = xuid.clone();
                let expired = *expires_at <= now_secs() + 120 || session.expired();
                auth::set_current(session);
                if expired && allow_refresh {
                    if let Some(cid) = self.store.resolved_client_id() {
                        self.start_microsoft_refresh(cid, refresh_token.clone());
                    } else {
                        self.status = format!("Signed in as {name} (token expired — sign in again)");
                    }
                } else {
                    self.status = format!("Active Microsoft account: {name}");
                }
            }
        }
    }

    /// Polls background Microsoft OAuth / skin upload threads.
    /// Returns `Some(new_player_name)` when the active account changed.
    pub fn poll(&mut self) -> Option<String> {
        let mut changed_name = None;
        let events: Vec<AuthEvent> = self
            .auth_rx
            .as_ref()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).collect())
            .unwrap_or_default();
        let mut finished = false;
        for ev in events {
            match ev {
                AuthEvent::DeviceCode(prompt) => {
                    self.status = format!("Enter code {} at {}", prompt.user_code, prompt.verification_uri);
                    #[cfg(target_os = "android")]
                    crate::android::open_url(&prompt.verification_uri);
                    self.device_prompt = Some(prompt);
                }
                AuthEvent::Status(msg) => {
                    self.status = msg;
                }
                AuthEvent::Success(account) => {
                    let name = account.name().to_owned();
                    self.upsert_account(account.clone());
                    self.activate_account_inner(&account, false);
                    self.device_prompt = None;
                    self.status = format!("Signed in as {name} (Microsoft)");
                    changed_name = Some(name);
                    finished = true;
                }
                AuthEvent::Error(err) => {
                    self.device_prompt = None;
                    self.status = err;
                    finished = true;
                }
            }
        }
        if finished {
            self.auth_rx = None;
            self.auth_cancel = None;
        }
        if let Some(rx) = &self.upload_rx
            && let Ok(UploadEvent::Done(res)) = rx.try_recv()
        {
            self.upload_rx = None;
            self.status = match res {
                Ok(msg) => msg,
                Err(err) => err,
            };
        }
        changed_name
    }

    fn upsert_account(&mut self, account: SavedAccount) {
        let existing = self.store.accounts.iter().position(|a| match (a, &account) {
            (SavedAccount::Microsoft { uuid: u1, .. }, SavedAccount::Microsoft { uuid: u2, .. }) => {
                u1.eq_ignore_ascii_case(u2)
            }
            (SavedAccount::Offline { name: n1 }, SavedAccount::Offline { name: n2 }) => {
                n1.eq_ignore_ascii_case(n2)
            }
            _ => false,
        });
        let idx = if let Some(i) = existing {
            self.store.accounts[i] = account;
            i
        } else {
            self.store.accounts.push(account);
            self.store.accounts.len() - 1
        };
        self.store.selected = Some(idx);
        self.store.save();
    }

    fn reload_skins(&mut self) {
        let mut entries = vec![SkinEntry {
            file: None,
            label: "Default (Steve / Alex)".into(),
            rgba: None,
        }];
        let dir = SkinSelection::dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(read) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = read
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                })
                .collect();
            files.sort();
            for path in files {
                if let Ok(rgba) = decode_skin_file(&path)
                    && let Some(fname) = path.file_name().and_then(|n| n.to_str())
                {
                    let label = fname
                        .strip_suffix(".png")
                        .or_else(|| fname.strip_suffix(".PNG"))
                        .unwrap_or(fname)
                        .to_owned();
                    entries.push(SkinEntry {
                        file: Some(fname.to_owned()),
                        label,
                        rgba: Some(rgba),
                    });
                }
            }
        }
        self.skins = entries;
        self.skin_cursor = self
            .selection
            .file
            .as_deref()
            .and_then(|f| self.skins.iter().position(|e| e.file.as_deref() == Some(f)))
            .unwrap_or(0);
        self.skin_page = self.skin_cursor / 6;
        self.preview_slim = self.selection.slim;
    }

    fn import_from_downloads(&mut self) {
        let dest = SkinSelection::dir();
        let _ = std::fs::create_dir_all(&dest);
        let mut search_dirs: Vec<PathBuf> = vec![PathBuf::from("/sdcard/Download")];
        if let Some(home) = std::env::var_os("HOME") {
            search_dirs.push(PathBuf::from(home).join("Downloads"));
        }
        let mut imported = 0usize;
        for src_dir in search_dirs {
            let Ok(read) = std::fs::read_dir(&src_dir) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if !path.is_file()
                    || !path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                {
                    continue;
                }
                if decode_skin_file(&path).is_ok()
                    && let Some(name) = path.file_name()
                {
                    let target = dest.join(name);
                    if std::fs::copy(&path, &target).is_ok() {
                        imported += 1;
                    }
                }
            }
        }
        self.reload_skins();
        if imported > 0 {
            self.status = format!("Imported {imported} skin(s) from Downloads");
        } else {
            self.status = "No 64x64 / 64x32 PNG skins found in Downloads".into();
        }
    }

    fn start_microsoft_login(&mut self, client_id: String) {
        self.cancel_auth();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.auth_rx = Some(rx);
        self.auth_cancel = Some(cancel);
        self.device_prompt = None;
        self.status = "Requesting Microsoft device code...".into();
        std::thread::spawn(move || {
            let res = run_microsoft_device_flow(&client_id, &tx, &cancel_bg);
            match res {
                Ok(acc) => {
                    let _ = tx.send(AuthEvent::Success(acc));
                }
                Err(err) => {
                    let _ = tx.send(AuthEvent::Error(err));
                }
            }
        });
    }

    fn start_microsoft_refresh(&mut self, client_id: String, refresh_token: String) {
        self.cancel_auth();
        let (tx, rx) = mpsc::channel();
        self.auth_rx = Some(rx);
        self.status = "Refreshing Microsoft session...".into();
        std::thread::spawn(move || match refresh_microsoft_account(&client_id, &refresh_token) {
            Ok(acc) => {
                let _ = tx.send(AuthEvent::Success(acc));
            }
            Err(err) => {
                let _ = tx.send(AuthEvent::Error(err));
            }
        });
    }

    fn cancel_auth(&mut self) {
        if let Some(cancel) = self.auth_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.auth_rx = None;
        self.device_prompt = None;
    }

    fn start_skin_upload(&mut self) {
        let Some(session) = auth::current() else {
            self.status = "Sign in with a Microsoft account first to upload.".into();
            return;
        };
        let Some(file) = self.skins.get(self.skin_cursor).and_then(|s| s.file.clone()) else {
            self.status = "Select a custom PNG skin to upload.".into();
            return;
        };
        let path = SkinSelection::dir().join(&file);
        let Ok(png_bytes) = std::fs::read(&path) else {
            self.status = "Could not read skin PNG.".into();
            return;
        };
        let slim = self.preview_slim;
        let token = session.access_token().to_owned();
        let (tx, rx) = mpsc::channel();
        self.upload_rx = Some(rx);
        self.status = "Uploading skin to Microsoft account...".into();
        std::thread::spawn(move || {
            let res = upload_minecraft_skin(&token, &png_bytes, slim);
            let _ = tx.send(UploadEvent::Done(res));
        });
    }

    fn set_focus(&mut self, field: TextField) {
        self.focused = field;
        #[cfg(target_os = "android")]
        crate::android::show_keyboard(field != TextField::None);
    }

    pub fn close(&mut self) {
        self.page = ModalPage::None;
        self.set_focus(TextField::None);
    }

    /// Draws the top-left Account & Skins buttons on the Title Screen, and the active modal if open.
    pub fn draw(
        &mut self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        current_player_name: &str,
        cursor: (f32, f32),
    ) {
        self.buttons.clear();
        let (gw, gh) = (batch.width, batch.height);

        // Compact top-left buttons on the Title Screen (when no modal is open).
        if self.page == ModalPage::None {
            let is_ms = auth::current().is_some();
            let badge = if is_ms { "MS" } else { "Offline" };
            let name = self.active_name().unwrap_or(current_player_name);
            let acc_label = format!("{badge}: {name}");
            let acc_w = (HudBatch::text_width(atlas, &acc_label) + 14.0).clamp(72.0, 145.0);
            let acc_rect = [6.0, 6.0, acc_w, 18.0];
            let skins_rect = [6.0 + acc_w + 4.0, 6.0, 44.0, 18.0];
            draw_btn(batch, atlas, acc_rect, &acc_label, is_ms, hit(acc_rect, cursor));
            draw_btn(batch, atlas, skins_rect, "Skins", false, hit(skins_rect, cursor));
            self.buttons.push((acc_rect, UiBtn::OpenAccounts));
            self.buttons.push((skins_rect, UiBtn::OpenSkins));
            return;
        }

        // Modal backdrop.
        batch.fill(atlas, 0.0, 0.0, gw, gh, rgba(0, 0, 0, 175));
        let panel_w = (gw - 24.0).clamp(260.0, 360.0);
        let panel_h = (gh - 20.0).clamp(192.0, 238.0);
        let px = ((gw - panel_w) * 0.5).floor();
        let py = ((gh - panel_h) * 0.5).floor();
        draw_panel(batch, atlas, px, py, panel_w, panel_h);

        match self.page {
            ModalPage::Accounts => self.draw_accounts_modal(batch, atlas, px, py, panel_w, panel_h, cursor),
            ModalPage::Skins => self.draw_skins_modal(batch, atlas, px, py, panel_w, panel_h, cursor),
            ModalPage::None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_accounts_modal(
        &mut self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        px: f32,
        py: f32,
        pw: f32,
        ph: f32,
        cursor: (f32, f32),
    ) {
        let pad = 10.0;
        let title = "Accounts (Microsoft & Offline)";
        let tw = HudBatch::text_width(atlas, title);
        batch.text(atlas, title, px + (pw - tw) * 0.5, py + 7.0, rgba(255, 255, 255, 255), true);

        let close_rect = [px + pw - 24.0, py + 5.0, 18.0, 14.0];
        draw_btn(batch, atlas, close_rect, "X", false, hit(close_rect, cursor));
        self.buttons.push((close_rect, UiBtn::CloseModal));

        let mut y = py + 23.0;
        let inner_w = pw - pad * 2.0;

        // Saved accounts list (up to 4 visible rows).
        batch.text(atlas, "Saved Accounts (tap to switch):", px + pad, y, rgba(185, 195, 210, 255), true);
        y += 10.0;
        if self.store.accounts.is_empty() {
            batch.text(atlas, "No saved accounts yet.", px + pad + 4.0, y + 3.0, rgba(145, 150, 165, 255), true);
            y += 18.0;
        } else {
            let max_rows = 4usize.min(self.store.accounts.len());
            for idx in 0..max_rows {
                let acc = &self.store.accounts[idx];
                let selected = self.store.selected == Some(idx);
                let tag = if acc.is_microsoft() { "[MS]" } else { "[Offline]" };
                let mark = if selected { "* " } else { "" };
                let label = format!("{mark}{tag} {}", acc.name());
                let row_rect = [px + pad, y, inner_w - 22.0, 16.0];
                let del_rect = [px + pw - pad - 18.0, y, 18.0, 16.0];
                draw_btn(batch, atlas, row_rect, &label, selected, hit(row_rect, cursor));
                draw_btn(batch, atlas, del_rect, "-", false, hit(del_rect, cursor));
                self.buttons.push((row_rect, UiBtn::SelectAccount(idx)));
                self.buttons.push((del_rect, UiBtn::DeleteAccount(idx)));
                y += 18.0;
            }
        }
        y += 3.0;

        // Offline account entry.
        batch.text(atlas, "Add Offline Account:", px + pad, y, rgba(185, 195, 210, 255), true);
        y += 10.0;
        let input_w = inner_w - 66.0;
        let input_rect = [px + pad, y, input_w, 17.0];
        let add_rect = [px + pad + input_w + 4.0, y, 62.0, 17.0];
        let focused_off = self.focused == TextField::OfflineName;
        draw_input(batch, atlas, input_rect, &self.offline_input, "Player name...", focused_off);
        draw_btn(batch, atlas, add_rect, "Add Offline", false, hit(add_rect, cursor));
        self.buttons.push((input_rect, UiBtn::FocusOfflineName));
        self.buttons.push((add_rect, UiBtn::AddOffline));
        y += 22.0;

        // Microsoft sign-in section.
        batch.text(atlas, "Microsoft Account:", px + pad, y, rgba(185, 195, 210, 255), true);
        y += 10.0;
        if let Some(prompt) = &self.device_prompt {
            let code_line = format!("Code: {}   ({})", prompt.user_code, prompt.verification_uri);
            batch.text(atlas, &code_line, px + pad, y + 2.0, rgba(255, 232, 120, 255), true);
            y += 12.0;
            let open_rect = [px + pad, y, inner_w * 0.5 - 2.0, 17.0];
            let cancel_rect = [px + pad + inner_w * 0.5 + 2.0, y, inner_w * 0.5 - 2.0, 17.0];
            draw_btn(batch, atlas, open_rect, "Open Browser", true, hit(open_rect, cursor));
            draw_btn(batch, atlas, cancel_rect, "Cancel", false, hit(cancel_rect, cursor));
            self.buttons.push((open_rect, UiBtn::OpenBrowser));
            self.buttons.push((cancel_rect, UiBtn::CancelMicrosoft));
        } else {
            let has_env_cid = std::env::var("HORIZON_MSA_CLIENT_ID")
                .ok()
                .or_else(|| option_env!("HORIZON_MSA_CLIENT_ID").map(str::to_owned))
                .is_some_and(|s| !s.trim().is_empty());
            if !has_env_cid {
                let cid_rect = [px + pad, y, inner_w, 16.0];
                draw_input(
                    batch,
                    atlas,
                    cid_rect,
                    &self.client_id_input,
                    "Azure Client ID (HORIZON_MSA_CLIENT_ID)",
                    self.focused == TextField::ClientId,
                );
                self.buttons.push((cid_rect, UiBtn::FocusClientId));
                y += 19.0;
            }
            let ms_rect = [px + pad, y, inner_w, 18.0];
            let label = if self.auth_rx.is_some() {
                "Signing in... (tap to cancel)"
            } else {
                "Sign in with Microsoft"
            };
            draw_btn(batch, atlas, ms_rect, label, true, hit(ms_rect, cursor));
            self.buttons.push((
                ms_rect,
                if self.auth_rx.is_some() {
                    UiBtn::CancelMicrosoft
                } else {
                    UiBtn::StartMicrosoft
                },
            ));
        }

        // Status line at bottom.
        if !self.status.is_empty() {
            let sy = py + ph - 14.0;
            let start = batch.vertices.len();
            let sw = HudBatch::text_width(atlas, &self.status);
            batch.text(atlas, &self.status, px + pad, sy, rgba(170, 225, 185, 255), true);
            if sw > inner_w {
                let s = (inner_w / sw).clamp(0.5, 1.0);
                batch.scale_since(start, px + pad, sy + 4.0, s, s);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_skins_modal(
        &mut self,
        batch: &mut HudBatch,
        atlas: &HudAtlas,
        px: f32,
        py: f32,
        pw: f32,
        ph: f32,
        cursor: (f32, f32),
    ) {
        let pad = 10.0;
        let title = "Skin Selector";
        let tw = HudBatch::text_width(atlas, title);
        batch.text(atlas, title, px + (pw - tw) * 0.5, py + 7.0, rgba(255, 255, 255, 255), true);

        let close_rect = [px + pw - 24.0, py + 5.0, 18.0, 14.0];
        draw_btn(batch, atlas, close_rect, "X", false, hit(close_rect, cursor));
        self.buttons.push((close_rect, UiBtn::CloseModal));

        let left_w = (pw - pad * 3.0) * 0.56;
        let right_x = px + pad + left_w + pad;
        let right_w = pw - pad * 3.0 - left_w;

        // Left column: skin list (6 items per page).
        let mut y = py + 24.0;
        let per_page = 6usize;
        let total_pages = self.skins.len().max(1).div_ceil(per_page);
        self.skin_page = self.skin_page.min(total_pages - 1);
        let start_idx = self.skin_page * per_page;
        let end_idx = (start_idx + per_page).min(self.skins.len());
        for idx in start_idx..end_idx {
            let entry = &self.skins[idx];
            let selected = idx == self.skin_cursor;
            let active = entry.file == self.selection.file;
            let prefix = if active { "* " } else { "" };
            let label = format!("{prefix}{}", entry.label);
            let rect = [px + pad, y, left_w, 16.0];
            draw_btn(batch, atlas, rect, &label, selected, hit(rect, cursor));
            self.buttons.push((rect, UiBtn::SelectSkin(idx)));
            y += 18.0;
        }

        // Pagination + Import buttons under list.
        let nav_y = py + ph - 52.0;
        if total_pages > 1 {
            let prev_rect = [px + pad, nav_y, 26.0, 15.0];
            let next_rect = [px + pad + left_w - 26.0, nav_y, 26.0, 15.0];
            draw_btn(batch, atlas, prev_rect, "<", false, hit(prev_rect, cursor));
            draw_btn(batch, atlas, next_rect, ">", false, hit(next_rect, cursor));
            let pg = format!("{}/{}", self.skin_page + 1, total_pages);
            let pgw = HudBatch::text_width(atlas, &pg);
            batch.text(atlas, &pg, px + pad + (left_w - pgw) * 0.5, nav_y + 4.0, rgba(210, 215, 225, 255), true);
            self.buttons.push((prev_rect, UiBtn::SkinsPrev));
            self.buttons.push((next_rect, UiBtn::SkinsNext));
        }

        let import_rect = [px + pad, py + ph - 33.0, left_w, 17.0];
        draw_btn(batch, atlas, import_rect, "Import Downloads", false, hit(import_rect, cursor));
        self.buttons.push((import_rect, UiBtn::ImportDownloads));

        // Right column: live 2D skin preview + Classic/Slim + Apply + Upload.
        let preview_h = 84.0;
        let preview_rect = [right_x, py + 24.0, right_w, preview_h];
        batch.fill(atlas, preview_rect[0], preview_rect[1], preview_rect[2], preview_rect[3], rgba(26, 30, 40, 235));
        if let Some(rgba_pixels) = self.skins.get(self.skin_cursor).and_then(|s| s.rgba.as_deref()) {
            draw_skin_preview(batch, atlas, preview_rect, rgba_pixels, self.preview_slim);
        } else {
            let msg = "Vanilla Default";
            let mw = HudBatch::text_width(atlas, msg);
            batch.text(
                atlas,
                msg,
                preview_rect[0] + (preview_rect[2] - mw) * 0.5,
                preview_rect[1] + preview_h * 0.5 - 4.0,
                rgba(180, 190, 205, 255),
                true,
            );
        }

        let mut ry = py + 24.0 + preview_h + 6.0;
        let slim_rect = [right_x, ry, right_w, 16.0];
        let slim_label = if self.preview_slim { "Model: Slim (Alex)" } else { "Model: Classic (Steve)" };
        draw_btn(batch, atlas, slim_rect, slim_label, self.preview_slim, hit(slim_rect, cursor));
        self.buttons.push((slim_rect, UiBtn::ToggleSlim));
        ry += 19.0;

        let apply_rect = [right_x, ry, right_w, 17.0];
        draw_btn(batch, atlas, apply_rect, "Apply Skin", true, hit(apply_rect, cursor));
        self.buttons.push((apply_rect, UiBtn::ApplySkin));
        ry += 20.0;

        if auth::current().is_some()
            && self.skins.get(self.skin_cursor).and_then(|s| s.file.as_ref()).is_some()
        {
            let up_rect = [right_x, ry, right_w, 17.0];
            draw_btn(batch, atlas, up_rect, "Upload to MS", false, hit(up_rect, cursor));
            self.buttons.push((up_rect, UiBtn::UploadSkin));
        }

        if !self.status.is_empty() {
            let sy = py + ph - 14.0;
            let inner_w = pw - pad * 2.0;
            let start = batch.vertices.len();
            let sw = HudBatch::text_width(atlas, &self.status);
            batch.text(atlas, &self.status, px + pad, sy, rgba(170, 225, 185, 255), true);
            if sw > inner_w {
                let s = (inner_w / sw).clamp(0.5, 1.0);
                batch.scale_since(start, px + pad, sy + 4.0, s, s);
            }
        }
    }

    /// Handles a left-click in GUI coordinates `(x, y)`.
    /// Returns `(consumed, Option<new_player_name>)`.
    pub fn click(&mut self, x: f32, y: f32) -> (bool, Option<String>) {
        let clicked = self.buttons.iter().find(|(r, _)| hit(*r, (x, y))).map(|(_, b)| *b);
        let Some(btn) = clicked else {
            if self.is_open() {
                self.set_focus(TextField::None);
                return (true, None);
            }
            return (false, None);
        };
        let mut new_name = None;
        match btn {
            UiBtn::OpenAccounts => {
                self.page = ModalPage::Accounts;
                self.set_focus(TextField::None);
            }
            UiBtn::OpenSkins => {
                self.reload_skins();
                self.page = ModalPage::Skins;
                self.set_focus(TextField::None);
            }
            UiBtn::CloseModal => {
                self.close();
            }
            UiBtn::SelectAccount(idx) => {
                if let Some(acc) = self.store.accounts.get(idx).cloned() {
                    self.store.selected = Some(idx);
                    self.store.save();
                    self.activate_account_inner(&acc, true);
                    new_name = Some(acc.name().to_owned());
                }
            }
            UiBtn::DeleteAccount(idx) => {
                if idx < self.store.accounts.len() {
                    let removed_active = self.store.selected == Some(idx);
                    self.store.accounts.remove(idx);
                    self.store.selected = match self.store.selected {
                        Some(sel) if sel == idx => None,
                        Some(sel) if sel > idx => Some(sel - 1),
                        other => other,
                    };
                    self.store.save();
                    if removed_active {
                        auth::clear_current();
                        self.status = "Account removed.".into();
                    }
                }
            }
            UiBtn::FocusOfflineName => {
                self.set_focus(TextField::OfflineName);
            }
            UiBtn::AddOffline => {
                let name = self.offline_input.trim().to_owned();
                if name.is_empty() || name.chars().count() > 16 || !name.chars().all(valid_name_char) {
                    self.status = "Name must be 1-16 letters, digits, or '_'.".into();
                } else {
                    let acc = SavedAccount::Offline { name: name.clone() };
                    self.upsert_account(acc.clone());
                    self.activate_account_inner(&acc, false);
                    self.offline_input.clear();
                    self.set_focus(TextField::None);
                    new_name = Some(name);
                }
            }
            UiBtn::FocusClientId => {
                self.set_focus(TextField::ClientId);
            }
            UiBtn::StartMicrosoft => {
                if !self.client_id_input.trim().is_empty() {
                    self.store.msa_client_id = Some(self.client_id_input.trim().to_owned());
                    self.store.save();
                }
                if let Some(cid) = self.store.resolved_client_id() {
                    self.set_focus(TextField::None);
                    self.start_microsoft_login(cid);
                } else {
                    self.status = "Enter an Azure Client ID (or set HORIZON_MSA_CLIENT_ID).".into();
                    self.set_focus(TextField::ClientId);
                }
            }
            UiBtn::CancelMicrosoft => {
                self.cancel_auth();
                self.status = "Microsoft sign-in cancelled.".into();
            }
            UiBtn::OpenBrowser => {
                if let Some(prompt) = &self.device_prompt {
                    #[cfg(target_os = "android")]
                    crate::android::open_url(&prompt.verification_uri);
                    self.status = format!("Visit {} and enter {}", prompt.verification_uri, prompt.user_code);
                }
            }
            UiBtn::SelectSkin(idx) => {
                if idx < self.skins.len() {
                    self.skin_cursor = idx;
                }
            }
            UiBtn::ToggleSlim => {
                self.preview_slim = !self.preview_slim;
            }
            UiBtn::ImportDownloads => {
                self.import_from_downloads();
            }
            UiBtn::ApplySkin => {
                if let Some(entry) = self.skins.get(self.skin_cursor).cloned() {
                    self.selection.file = entry.file.clone();
                    self.selection.slim = self.preview_slim;
                    self.selection.save();
                    match entry.rgba {
                        Some(rgba) => {
                            skins::set_offline_skin(Some((rgba, self.preview_slim)));
                            self.status = format!("Applied skin: {}", entry.label);
                        }
                        None => {
                            skins::set_offline_skin(None);
                            self.status = "Applied vanilla default skin.".into();
                        }
                    }
                }
            }
            UiBtn::UploadSkin => {
                self.start_skin_upload();
            }
            UiBtn::SkinsPrev => {
                self.skin_page = self.skin_page.saturating_sub(1);
            }
            UiBtn::SkinsNext => {
                self.skin_page += 1;
            }
        }
        (true, new_name)
    }

    /// Handles keyboard input while a modal is open.
    /// Returns `(consumed, Option<new_player_name>)`.
    pub fn key(&mut self, code: Option<KeyCode>, text: Option<&str>) -> (bool, Option<String>) {
        if !self.is_open() {
            return (false, None);
        }
        match code {
            Some(KeyCode::Escape) => {
                self.close();
                return (true, None);
            }
            Some(KeyCode::Backspace) => {
                match self.focused {
                    TextField::OfflineName => {
                        self.offline_input.pop();
                    }
                    TextField::ClientId => {
                        self.client_id_input.pop();
                    }
                    TextField::None => {}
                }
                return (true, None);
            }
            Some(KeyCode::Enter | KeyCode::NumpadEnter) => {
                if self.focused == TextField::OfflineName {
                    let name = self.offline_input.trim().to_owned();
                    if !name.is_empty() && name.chars().count() <= 16 && name.chars().all(valid_name_char) {
                        let acc = SavedAccount::Offline { name: name.clone() };
                        self.upsert_account(acc.clone());
                        self.activate_account_inner(&acc, false);
                        self.offline_input.clear();
                        self.set_focus(TextField::None);
                        return (true, Some(name));
                    }
                }
                self.set_focus(TextField::None);
                return (true, None);
            }
            _ => {}
        }
        if let Some(s) = text {
            match self.focused {
                TextField::OfflineName => {
                    for ch in s.chars() {
                        if valid_name_char(ch) && self.offline_input.chars().count() < 16 {
                            self.offline_input.push(ch);
                        }
                    }
                }
                TextField::ClientId => {
                    for ch in s.chars() {
                        if !ch.is_control() && self.client_id_input.len() < 64 {
                            self.client_id_input.push(ch);
                        }
                    }
                }
                TextField::None => {}
            }
        }
        (true, None)
    }
}

fn valid_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn offline_uuid(name: &str) -> u128 {
    horizon_protocol::crypto::offline_uuid(name)
}

pub fn decode_skin_file(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (w, h, rgba) = hud::decode_bytes(&bytes).map_err(|e| e.to_string())?;
    skins::process_legacy_skin(w, h, &rgba).ok_or_else(|| "not a 64x64 or 64x32 skin".into())
}

// ----- 2D pixel-art skin front preview (adapted from Starling's skin_head / wardrobe) ------------

fn draw_skin_preview(b: &mut HudBatch, a: &HudAtlas, rect: [f32; 4], rgba_64: &[u8], slim: bool) {
    if rgba_64.len() < 64 * 64 * 4 {
        return;
    }
    // Character is 16px wide (4 arm + 8 body + 4 arm) x 32px tall (8 head + 12 body + 12 legs).
    let px = ((rect[2] - 8.0) / 16.0).min((rect[3] - 8.0) / 32.0).max(1.0);
    let ox = rect[0] + (rect[2] - 16.0 * px) * 0.5;
    let oy = rect[1] + (rect[3] - 32.0 * px) * 0.5;

    let mut blit = |sx: usize, sy: usize, sw: usize, sh: usize, dx: f32, dy: f32, overlay: bool| {
        for py in 0..sh {
            for px_i in 0..sw {
                let idx = ((sy + py) * 64 + (sx + px_i)) * 4;
                let alpha = rgba_64[idx + 3];
                if overlay && alpha < 32 {
                    continue;
                }
                let a_out = if overlay { alpha } else { 255 };
                let col = rgba(rgba_64[idx], rgba_64[idx + 1], rgba_64[idx + 2], a_out);
                b.fill(
                    a,
                    ox + (dx + px_i as f32) * px,
                    oy + (dy + py as f32) * px,
                    px + 0.15,
                    px + 0.15,
                    col,
                );
            }
        }
    };

    // Head + hat layer.
    blit(8, 8, 8, 8, 4.0, 0.0, false);
    blit(40, 8, 8, 8, 4.0, 0.0, true);
    // Torso + jacket layer.
    blit(20, 20, 8, 12, 4.0, 8.0, false);
    blit(20, 36, 8, 12, 4.0, 8.0, true);
    // Right & Left arms (classic 4px or slim 3px).
    let aw = if slim { 3 } else { 4 };
    let right_arm_x = if slim { 1.0 } else { 0.0 };
    blit(44, 20, aw, 12, right_arm_x, 8.0, false);
    blit(44, 36, aw, 12, right_arm_x, 8.0, true);
    blit(36, 52, aw, 12, 12.0, 8.0, false);
    blit(52, 52, aw, 12, 12.0, 8.0, true);
    // Right & Left legs + pants layer.
    blit(4, 20, 4, 12, 4.0, 20.0, false);
    blit(4, 36, 4, 12, 4.0, 20.0, true);
    blit(20, 52, 4, 12, 8.0, 20.0, false);
    blit(4, 52, 4, 12, 8.0, 20.0, true);
}

// ----- Microsoft OAuth 2.0 Device Code -> Xbox Live -> XSTS -> Minecraft -------------------------

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
}

fn post_form(url: &str, fields: &[(&str, &str)], service: &str) -> Result<(u16, Value), String> {
    let resp = match http_agent().post(url).send_form(fields) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return Err(format!("Could not reach {service}. Check internet connection.")),
    };
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn post_json(url: &str, body: &Value, service: &str) -> Result<(u16, Value), String> {
    let payload = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let resp = match http_agent()
        .post(url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .send_string(&payload)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return Err(format!("Could not reach {service}. Check internet connection.")),
    };
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn get_json_bearer(url: &str, token: &str, service: &str) -> Result<(u16, Value), String> {
    let resp = match http_agent()
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return Err(format!("Could not reach {service}. Check internet connection.")),
    };
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn run_microsoft_device_flow(
    client_id: &str,
    tx: &mpsc::Sender<AuthEvent>,
    cancel: &AtomicBool,
) -> Result<SavedAccount, String> {
    let (status, v) = post_form(
        &format!("{MSA_ENDPOINT}/devicecode"),
        &[("client_id", client_id), ("scope", MSA_SCOPE)],
        "Microsoft",
    )?;
    let (Some(user_code), Some(device_code)) = (
        str_field(&v, "user_code").filter(|_| status == 200),
        str_field(&v, "device_code"),
    ) else {
        let err = str_field(&v, "error").unwrap_or_else(|| format!("HTTP {status}"));
        return Err(format!("Microsoft sign-in failed ({err})."));
    };
    let verification_uri = str_field(&v, "verification_uri")
        .unwrap_or_else(|| "https://www.microsoft.com/link".into());
    let mut interval = v.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 30);
    let expires_at = now_secs()
        + v.get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(900)
            .min(3600);

    let _ = tx.send(AuthEvent::DeviceCode(DevicePrompt {
        user_code,
        verification_uri,
    }));

    let (ms_access, ms_refresh) = loop {
        let until = std::time::Instant::now() + Duration::from_secs(interval);
        while std::time::Instant::now() < until {
            if cancel.load(Ordering::Relaxed) {
                return Err("Microsoft sign-in cancelled.".into());
            }
            std::thread::sleep(Duration::from_millis(120));
        }
        if now_secs() > expires_at {
            return Err("Sign-in code expired. Try again.".into());
        }
        let (st, tv) = post_form(
            &format!("{MSA_ENDPOINT}/token"),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", &device_code),
            ],
            "Microsoft",
        )?;
        match tv.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval = (interval + 5).min(60);
                continue;
            }
            Some("authorization_declined") => return Err("Sign-in declined in browser.".into()),
            Some("expired_token") => return Err("Sign-in code expired.".into()),
            Some(other) => return Err(format!("Microsoft OAuth error: {other}")),
            None => {}
        }
        if st == 200
            && let (Some(access), Some(refresh)) =
                (str_field(&tv, "access_token"), str_field(&tv, "refresh_token"))
        {
            break (access, refresh);
        }
        return Err(format!("Unexpected Microsoft token response (HTTP {st})."));
    };

    let _ = tx.send(AuthEvent::Status("Authenticating with Xbox Live...".into()));
    exchange_xbox_and_minecraft(&ms_access, &ms_refresh, Some(tx))
}

fn refresh_microsoft_account(client_id: &str, refresh_token: &str) -> Result<SavedAccount, String> {
    let (st, v) = post_form(
        &format!("{MSA_ENDPOINT}/token"),
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("scope", MSA_SCOPE),
        ],
        "Microsoft",
    )?;
    let (Some(access), Some(new_refresh)) = (
        str_field(&v, "access_token").filter(|_| st == 200),
        str_field(&v, "refresh_token"),
    ) else {
        return Err("Microsoft session expired. Please sign in again.".into());
    };
    exchange_xbox_and_minecraft(&access, &new_refresh, None)
}

fn exchange_xbox_and_minecraft(
    ms_access: &str,
    ms_refresh: &str,
    tx: Option<&mpsc::Sender<AuthEvent>>,
) -> Result<SavedAccount, String> {
    let (status, xbl) = post_json(
        XBL_ENDPOINT,
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
        "Xbox Live",
    )?;
    let xbl_token = str_field(&xbl, "Token")
        .filter(|_| status == 200)
        .ok_or("Xbox Live refused the sign-in.")?;

    let (status, xsts) = post_json(
        XSTS_ENDPOINT,
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
        "Xbox Live",
    )?;
    if let Some(code) = xsts.get("XErr").and_then(Value::as_u64) {
        return Err(match code {
            2148916233 => "No Xbox profile yet. Sign in at xbox.com first.".into(),
            2148916235 => "Xbox Live is unavailable in your region.".into(),
            2148916238 => "Child account requires Microsoft family setup.".into(),
            other => format!("Xbox Live error {other}."),
        });
    }
    let claims = xsts.pointer("/DisplayClaims/xui/0");
    let (Some(xsts_token), Some(uhs)) = (
        str_field(&xsts, "Token").filter(|_| status == 200),
        claims.and_then(|c| str_field(c, "uhs")),
    ) else {
        return Err("Xbox Live XSTS failed.".into());
    };
    let xuid = claims.and_then(|c| str_field(c, "xid"));

    if let Some(tx) = tx {
        let _ = tx.send(AuthEvent::Status("Signing into Minecraft Services...".into()));
    }
    let xtoken = format!("XBL3.0 x={uhs};{xsts_token}");
    let (mc_token, expires_in) = match post_json(
        &format!("{MC_API}/launcher/login"),
        &json!({ "xtoken": xtoken, "platform": "PC_LAUNCHER" }),
        "Minecraft Services",
    ) {
        Ok((200, v)) if str_field(&v, "access_token").is_some() => (
            str_field(&v, "access_token").unwrap(),
            v.get("expires_in").and_then(Value::as_u64).unwrap_or(86400),
        ),
        _ => {
            let (st, v) = post_json(
                &format!("{MC_API}/authentication/login_with_xbox"),
                &json!({ "identityToken": xtoken }),
                "Minecraft Services",
            )?;
            let tok = str_field(&v, "access_token")
                .filter(|_| st == 200)
                .ok_or("Minecraft Services refused the login (app client ID may need approval).")?;
            (
                tok,
                v.get("expires_in").and_then(Value::as_u64).unwrap_or(86400),
            )
        }
    };

    let (st, profile) = get_json_bearer(&format!("{MC_API}/minecraft/profile"), &mc_token, "Minecraft Services")?;
    let (Some(uuid), Some(name)) = (
        str_field(&profile, "id").filter(|_| st == 200),
        str_field(&profile, "name"),
    ) else {
        return Err("No Minecraft Java profile found on this Microsoft account.".into());
    };

    Ok(SavedAccount::Microsoft {
        name,
        uuid,
        xuid,
        refresh_token: ms_refresh.to_owned(),
        access_token: mc_token,
        expires_at: now_secs().saturating_add(expires_in.min(7 * 86_400)),
    })
}

fn upload_minecraft_skin(token: &str, png_bytes: &[u8], slim: bool) -> Result<String, String> {
    let boundary = "----HorizonSkinBoundary7a9c3e";
    let variant = if slim { "slim" } else { "classic" };
    let mut body = Vec::with_capacity(png_bytes.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{variant}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\n\
             Content-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(png_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let resp = http_agent()
        .post(&format!("{MC_API}/minecraft/profile/skins"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body);

    match resp {
        Ok(r) if (200..300).contains(&r.status()) => Ok("Skin uploaded to Microsoft profile!".into()),
        Ok(r) => Err(format!("Upload failed (HTTP {}).", r.status())),
        Err(ureq::Error::Status(401 | 403, _)) => Err("Sign in again to upload your skin.".into()),
        Err(ureq::Error::Status(st, _)) => Err(format!("Upload failed (HTTP {st}).")),
        Err(_) => Err("Could not reach Minecraft Services.".into()),
    }
}

// ----- Compact MC-styled UI primitives -----------------------------------------------------------

fn hit([x, y, w, h]: [f32; 4], (cx, cy): (f32, f32)) -> bool {
    cx >= x && cx <= x + w && cy >= y && cy <= y + h
}

fn draw_panel(b: &mut HudBatch, a: &HudAtlas, x: f32, y: f32, w: f32, h: f32) {
    b.fill(a, x, y, w, h, rgba(16, 18, 24, 244));
    let border = rgba(125, 136, 158, 235);
    b.fill(a, x, y, w, 1.0, border);
    b.fill(a, x, y + h - 1.0, w, 1.0, border);
    b.fill(a, x, y, 1.0, h, border);
    b.fill(a, x + w - 1.0, y, 1.0, h, border);
}

fn draw_btn(
    b: &mut HudBatch,
    a: &HudAtlas,
    [x, y, w, h]: [f32; 4],
    label: &str,
    accent: bool,
    hovered: bool,
) {
    let bg = if accent {
        rgba(56, 124, 80, 242)
    } else if hovered {
        rgba(84, 92, 114, 238)
    } else {
        rgba(48, 52, 64, 232)
    };
    let hi = if accent {
        rgba(126, 214, 150, 235)
    } else {
        rgba(138, 146, 164, 200)
    };
    let lo = rgba(18, 20, 26, 242);
    b.fill(a, x, y, w, h, lo);
    b.fill(a, x + 1.0, y + 1.0, (w - 2.0).max(1.0), (h - 2.0).max(1.0), bg);
    b.fill(a, x + 1.0, y + 1.0, (w - 2.0).max(1.0), 1.0, hi);
    b.fill(a, x + 1.0, y + 1.0, 1.0, (h - 2.0).max(1.0), hi);

    let tw = HudBatch::text_width(a, label);
    let start = b.vertices.len();
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    b.text(a, label, cx - tw * 0.5, cy - 4.0, rgba(248, 248, 252, 255), true);
    let scale = ((w - 6.0).max(1.0) / tw.max(1.0)).min(1.0);
    if scale < 1.0 {
        b.scale_since(start, cx, cy, scale, scale);
    }
}

fn draw_input(
    b: &mut HudBatch,
    a: &HudAtlas,
    [x, y, w, h]: [f32; 4],
    value: &str,
    placeholder: &str,
    focused: bool,
) {
    let border = if focused {
        rgba(120, 210, 150, 255)
    } else {
        rgba(105, 112, 128, 220)
    };
    b.fill(a, x, y, w, h, border);
    b.fill(a, x + 1.0, y + 1.0, (w - 2.0).max(1.0), (h - 2.0).max(1.0), rgba(14, 16, 22, 248));
    let shown = if value.is_empty() && !focused {
        placeholder.to_owned()
    } else if focused {
        format!("{value}_")
    } else {
        value.to_owned()
    };
    let col = if value.is_empty() && !focused {
        rgba(125, 132, 148, 255)
    } else {
        rgba(245, 245, 250, 255)
    };
    let start = b.vertices.len();
    let tw = HudBatch::text_width(a, &shown);
    b.text(a, &shown, x + 4.0, y + (h - 8.0) * 0.5, col, true);
    if tw > w - 8.0 {
        let s = ((w - 8.0).max(1.0) / tw).clamp(0.45, 1.0);
        b.scale_since(start, x + 4.0, y + h * 0.5, s, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_account_add_and_switch_works() {
        let mut ui = AccountsUi {
            page: ModalPage::Accounts,
            store: AccountStore::default(),
            selection: SkinSelection::default(),
            offline_input: "Builder42".into(),
            client_id_input: String::new(),
            focused: TextField::OfflineName,
            status: String::new(),
            device_prompt: None,
            auth_rx: None,
            auth_cancel: None,
            upload_rx: None,
            skins: Vec::new(),
            skin_cursor: 0,
            skin_page: 0,
            preview_slim: false,
            buttons: Vec::new(),
        };
        let (consumed, name) = ui.key(Some(KeyCode::Enter), None);
        assert!(consumed);
        assert_eq!(name.as_deref(), Some("Builder42"));
        assert_eq!(ui.store.active().map(SavedAccount::name), Some("Builder42"));
        assert!(auth::current().is_none());
    }

    #[test]
    fn skin_preview_renders_pixels_for_64x64_rgba() {
        let atlas = HudAtlas::default();
        let mut batch = HudBatch::new([1280, 720], Default::default());
        let mut rgba_64 = vec![0u8; 64 * 64 * 4];
        for px in rgba_64.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 150, 100, 255]);
        }
        draw_skin_preview(&mut batch, &atlas, [10.0, 10.0, 100.0, 100.0], &rgba_64, true);
        assert!(!batch.vertices.is_empty());
    }
}
