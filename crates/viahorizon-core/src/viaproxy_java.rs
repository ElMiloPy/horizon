//! Wrapper around the official standalone Java **ViaProxy** (`RaphiMC/ViaProxy`),
//! which bundles the real ViaVersion + ViaBackwards + ViaRewind translators for full
//! Play-phase compatibility from Minecraft 1.7.2 through 26.3.

use crate::versions::ProtocolVersion;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Official ViaProxy GitHub release asset URL (latest release).
pub const VIAPROXY_RELEASE_URL: &str =
    "https://github.com/RaphiMC/ViaProxy/releases/latest/download/ViaProxy.jar";

/// Running standalone Java ViaProxy process managed by ViaHorizon.
pub struct ViaJavaProxy {
    pub listen_addr: String,
    pub target_addr: String,
    pub target_version: ProtocolVersion,
    child: Child,
}

impl ViaJavaProxy {
    /// Ensures `ViaProxy.jar` exists at `jar_path`, downloading it with `curl` if missing.
    pub fn ensure_jar(jar_path: &Path) -> io::Result<()> {
        if jar_path.is_file() && jar_path.metadata().is_ok_and(|m| m.len() > 1024 * 1024) {
            return Ok(());
        }
        if let Some(parent) = jar_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = jar_path.with_extension("jar.part");
        let status = Command::new("curl")
            .args(["-fL", "-o"])
            .arg(&tmp)
            .arg(VIAPROXY_RELEASE_URL)
            .status()?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to download ViaProxy.jar from {VIAPROXY_RELEASE_URL}"),
            ));
        }
        std::fs::rename(&tmp, jar_path)?;
        Ok(())
    }

    /// Launches `java -jar ViaProxy.jar cli ...` bound to a free localhost port (or `bind_addr`).
    pub fn start(
        java_bin: &Path,
        jar_path: &Path,
        bind_addr: Option<&str>,
        target_addr: &str,
        target_version: ProtocolVersion,
    ) -> io::Result<Self> {
        Self::ensure_jar(jar_path)?;
        let listen_addr = match bind_addr {
            Some(addr) => addr.to_owned(),
            None => {
                let probe = TcpListener::bind("127.0.0.1:0")?;
                probe.local_addr()?.to_string()
            }
        };
        let work_dir = jar_path.parent().unwrap_or_else(|| Path::new("."));
        let child = Command::new(java_bin)
            .current_dir(work_dir)
            .args([
                "-Xmx512M",
                "-jar",
            ])
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

/// Default path where `ViaProxy.jar` is cached (`<dir>/ViaProxy.jar`).
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
