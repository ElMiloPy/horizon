//! Auto-managed mini Android ARM64 JRE + official standalone Java **ViaProxy**
//! (`RaphiMC/ViaProxy`, bundling real ViaVersion + ViaBackwards + ViaRewind).

use crate::versions::ProtocolVersion;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Official ViaProxy GitHub release asset URL (latest release).
pub const VIAPROXY_RELEASE_URL: &str =
    "https://github.com/RaphiMC/ViaProxy/releases/latest/download/ViaProxy.jar";

/// Prebuilt OpenJDK 25 Android ARM64 JRE archive (for running ViaProxy on Android).
pub const ANDROID_JRE_URL: &str =
    "https://github.com/AngelAuraMC/angelauramc-openjdk-build/releases/download/download_jre25/jre25-android-arm64.tar.xz";

/// Running standalone Java ViaProxy process managed by ViaHorizon.
pub struct ViaJavaProxy {
    pub listen_addr: String,
    pub target_addr: String,
    pub target_version: ProtocolVersion,
    child: Child,
}

impl ViaJavaProxy {
    /// Returns a working `java` binary path:
    /// - If `<data_dir>/jre/bin/java` exists, uses it.
    /// - On Android, auto-downloads & extracts the ARM64 JRE into `<data_dir>/jre/`.
    /// - On desktop, falls back to `java` on `PATH`.
    pub fn ensure_jre(data_dir: &Path) -> io::Result<PathBuf> {
        let local_java = data_dir.join("jre").join("bin").join("java");
        if local_java.is_file() {
            return Ok(local_java);
        }
        if !cfg!(target_os = "android") {
            return Ok(PathBuf::from("java"));
        }
        let jre_dir = data_dir.join("jre");
        std::fs::create_dir_all(&jre_dir)?;
        let archive = data_dir.join("jre25-android-arm64.tar.xz");
        download_with_curl(ANDROID_JRE_URL, &archive)?;
        let status = Command::new("tar")
            .args(["-xJf"])
            .arg(&archive)
            .arg("-C")
            .arg(&jre_dir)
            .status()?;
        let _ = std::fs::remove_file(&archive);
        if !status.success() || !local_java.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Failed to extract Android ARM64 JRE",
            ));
        }
        Ok(local_java)
    }

    /// Ensures `ViaProxy.jar` exists at `<data_dir>/ViaProxy.jar`, downloading if missing.
    pub fn ensure_jar(data_dir: &Path) -> io::Result<PathBuf> {
        let jar_path = default_jar_path(data_dir);
        if jar_path.is_file() && jar_path.metadata().is_ok_and(|m| m.len() > 1024 * 1024) {
            return Ok(jar_path);
        }
        download_with_curl(VIAPROXY_RELEASE_URL, &jar_path)?;
        Ok(jar_path)
    }

    /// Auto-prepares JRE + `ViaProxy.jar` inside `data_dir` and launches the internal proxy.
    pub fn start_auto(
        data_dir: &Path,
        bind_addr: Option<&str>,
        target_addr: &str,
        target_version: ProtocolVersion,
    ) -> io::Result<Self> {
        let java_bin = Self::ensure_jre(data_dir)?;
        let jar_path = Self::ensure_jar(data_dir)?;
        Self::start(&java_bin, &jar_path, bind_addr, target_addr, target_version)
    }

    /// Launches `java -jar ViaProxy.jar cli ...` bound to a free localhost port (or `bind_addr`).
    pub fn start(
        java_bin: &Path,
        jar_path: &Path,
        bind_addr: Option<&str>,
        target_addr: &str,
        target_version: ProtocolVersion,
    ) -> io::Result<Self> {
        let listen_addr = match bind_addr {
            Some(addr) => addr.to_owned(),
            None => {
                let probe = TcpListener::bind("127.0.0.1:0")?;
                probe.local_addr()?.to_string()
            }
        };
        let work_dir = jar_path.parent().unwrap_or_else(|| Path::new("."));
        let mut cmd = Command::new(java_bin);
        cmd.current_dir(work_dir);
        if let Some(jre_root) = java_bin.parent().and_then(Path::parent) {
            let lib = jre_root.join("lib");
            let server = lib.join("server");
            if lib.is_dir() {
                cmd.env(
                    "LD_LIBRARY_PATH",
                    format!("{}:{}", lib.display(), server.display()),
                );
            }
        }
        let child = cmd
            .args(["-Xmx384M", "-jar"])
            .arg(jar_path)
            .args([
                "cli",
                "--bind-address",
                &listen_addr,
                "--target-address",
                target_addr,
                "--target-version",
                target_version.viaproxy_name(),
                "--auth-method",
                "NONE",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(Self {
            listen_addr,
            target_addr: target_addr.to_owned(),
            target_version,
            child,
        })
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ViaJavaProxy {
    fn drop(&mut self) {
        self.stop();
    }
}

fn download_with_curl(url: &str, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("part");
    let status = Command::new("curl")
        .args(["-fL", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to download {url}"),
        ));
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Default path where `ViaProxy.jar` is cached (`<data_dir>/ViaProxy.jar`).
pub fn default_jar_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ViaProxy.jar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_jar_path_ends_with_viaproxy_jar() {
        let p = default_jar_path(Path::new("/tmp/viahorizon"));
        assert_eq!(p, PathBuf::from("/tmp/viahorizon/ViaProxy.jar"));
    }
}
