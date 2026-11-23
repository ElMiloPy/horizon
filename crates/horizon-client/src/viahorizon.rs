//! ViaHorizon: ViaFabricPlus-style version switcher + auto-managed ViaProxy
//! (real ViaVersion + ViaBackwards + ViaRewind) for `horizon-client`.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const VIAPROXY_JAR_URL: &str =
    "https://github.com/RaphiMC/ViaProxy/releases/latest/download/ViaProxy.jar";

/// Supported target versions (index 0 = native 26.3, no proxy).
pub const VERSIONS: &[(&str, &str)] = &[
    ("26.3", "26.3"),
    ("1.21.4", "1.21.4"),
    ("1.20.4", "1.20.4"),
    ("1.19.4", "1.19.4"),
    ("1.18.2", "1.18.2"),
    ("1.16.5", "1.16.5"),
    ("1.12.2", "1.12.2"),
    ("1.8.9", "1.8.9"),
    ("1.7.10", "1.7.10"),
];

struct State {
    version_idx: usize,
    child: Option<Child>,
    active_key: Option<(String, usize)>,
    active_port: u16,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        let saved = load_saved_version();
        Mutex::new(State {
            version_idx: saved,
            child: None,
            active_key: None,
            active_port: 0,
        })
    })
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("HORIZON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("viahorizon")
}

fn load_saved_version() -> usize {
    let path = data_dir().join("version.txt");
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let trimmed = text.trim();
    VERSIONS
        .iter()
        .position(|(label, _)| *label == trimmed)
        .unwrap_or(0)
}

fn save_version(idx: usize) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("version.txt"), VERSIONS[idx].0);
}

/// Label shown on the Multiplayer screen button (e.g. `Via: 26.3` or `Via: 1.8.9`).
pub fn button_label() -> String {
    let idx = state().lock().map(|s| s.version_idx).unwrap_or(0);
    format!("Via: {}", VERSIONS[idx].0)
}

/// Cycle to the next target Minecraft version and persist it.
pub fn cycle_version() {
    if let Ok(mut s) = state().lock() {
        s.version_idx = (s.version_idx + 1) % VERSIONS.len();
        let idx = s.version_idx;
        save_version(idx);
    }
}

/// Resolve the address to connect to.
/// - If `26.3` (native) is selected, returns `target_addr` unchanged.
/// - Otherwise ensures `ViaProxy.jar` + a Java runtime are ready, starts
///   ViaProxy pointing at `target_addr`, and returns `127.0.0.1:<port>`.
pub fn prepare_address(target_addr: &str) -> String {
    let idx = state().lock().map(|s| s.version_idx).unwrap_or(0);
    if idx == 0 {
        return target_addr.to_owned();
    }
    let version = VERSIONS[idx].1;
    match start_or_reuse_proxy(target_addr, idx, version) {
        Ok(local_addr) => local_addr,
        Err(err) => {
            eprintln!("[viahorizon] failed to start ViaProxy for {version}: {err}");
            target_addr.to_owned()
        }
    }
}

fn start_or_reuse_proxy(target_addr: &str, idx: usize, version: &str) -> Result<String, String> {
    let mut s = state().lock().map_err(|_| "lock poisoned".to_owned())?;

    // Reuse already-running proxy for the same (target, version) if still alive.
    let same_key = s
        .active_key
        .as_ref()
        .is_some_and(|(prev_addr, prev_idx)| prev_addr == target_addr && *prev_idx == idx);
    if same_key
        && let Some(child) = s.child.as_mut()
        && child.try_wait().ok().flatten().is_none()
    {
        return Ok(format!("127.0.0.1:{}", s.active_port));
    }

    // Stop any previous proxy instance.
    if let Some(mut old) = s.child.take() {
        let _ = old.kill();
        let _ = old.wait();
    }
    s.active_key = None;

    let jar = ensure_viaproxy_jar()?;
    let java = find_java_bin()?;
    let port = pick_free_port()?;

    let mut cmd = Command::new(&java);
    cmd.arg("-Xmx256M")
        .arg("-jar")
        .arg(&jar)
        .arg("cli")
        .arg("--bind-address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--target-address")
        .arg(target_addr)
        .arg("--target-version")
        .arg(version)
        .arg("--auth-method")
        .arg("NONE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch `{}`: {e}", java.display()))?;

    // Wait briefly for ViaProxy to bind the local TCP port.
    let deadline = Instant::now() + Duration::from_secs(6);
    let bind_addr = format!("127.0.0.1:{port}");
    while Instant::now() < deadline {
        if TcpStream::connect(&bind_addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    s.child = Some(child);
    s.active_key = Some((target_addr.to_owned(), idx));
    s.active_port = port;
    Ok(bind_addr)
}

fn ensure_viaproxy_jar() -> Result<PathBuf, String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    let jar = dir.join("ViaProxy.jar");
    if jar.is_file() && jar.metadata().map(|m| m.len() > 1024).unwrap_or(false) {
        return Ok(jar);
    }

    let resp = ureq::get(VIAPROXY_JAR_URL)
        .call()
        .map_err(|e| format!("download ViaProxy.jar: {e}"))?;
    let mut reader = resp.into_reader();
    let tmp = dir.join("ViaProxy.jar.part");
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create tmp jar: {e}"))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| format!("write ViaProxy.jar: {e}"))?;
    file.flush().map_err(|e| format!("flush ViaProxy.jar: {e}"))?;
    std::fs::rename(&tmp, &jar).map_err(|e| format!("rename ViaProxy.jar: {e}"))?;
    Ok(jar)
}

fn find_java_bin() -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("VIAHORIZON_JAVA") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Ok(p);
        }
    }
    let bundled = data_dir().join("jre/bin/java");
    if bundled.is_file() {
        return Ok(bundled);
    }
    // Check if `java` is on PATH (desktop / Termux).
    if Command::new("java")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        return Ok(PathBuf::from("java"));
    }
    Err(format!(
        "no Java runtime found (set VIAHORIZON_JAVA or place JRE at {})",
        bundled.display()
    ))
}

fn pick_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind port: {e}"))?;
    Ok(listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port())
}
