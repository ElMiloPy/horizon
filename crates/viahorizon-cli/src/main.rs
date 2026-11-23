//! Standalone ViaHorizon proxy CLI (supports both minimal Rust translator and
//! full Java ViaProxy backend for 1.7.10 -> 26.3 Play-phase translation).
//!
//! Usage:
//!   viahorizon-cli --backend java --listen 127.0.0.1:25566 --target play.example.net:25565 --version 1.8.9

use std::path::PathBuf;
use viahorizon_core::{ProtocolVersion, ViaJavaProxy, ViaProxy};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut listen = "127.0.0.1:25566".to_owned();
    let mut target = None::<String>;
    let mut version = ProtocolVersion::V1_20_4;
    let mut backend = "java".to_owned();
    let mut java_bin = PathBuf::from("java");
    let mut jar_path = PathBuf::from("ViaProxy.jar");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--backend" if i + 1 < args.len() => {
                backend = args[i + 1].clone();
                i += 2;
            }
            "--java" if i + 1 < args.len() => {
                java_bin = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--jar" if i + 1 < args.len() => {
                jar_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--listen" if i + 1 < args.len() => {
                listen = args[i + 1].clone();
                i += 2;
            }
            "--target" if i + 1 < args.len() => {
                target = Some(args[i + 1].clone());
                i += 2;
            }
            "--version" if i + 1 < args.len() => {
                version = ProtocolVersion::from_name(&args[i + 1]).unwrap_or_else(|| {
                    eprintln!("Unknown version: {}", args[i + 1]);
                    std::process::exit(2);
                });
                i += 2;
            }
            "--list-versions" => {
                for v in ProtocolVersion::ALL {
                    println!("{v}");
                }
                return;
            }
            _ => i += 1,
        }
    }

    let Some(target) = target else {
        eprintln!(
            "Usage: viahorizon-cli [--backend java|rust] [--java <path>] [--jar ViaProxy.jar] \
             --listen 127.0.0.1:25566 --target <host:port> --version <1.7.10..26.3>"
        );
        std::process::exit(2);
    };

    if backend == "java" {
        let _proxy = ViaJavaProxy::start(&java_bin, &jar_path, Some(&listen), &target, version)
            .expect("start Java ViaProxy");
        println!(
            "ViaHorizon (Java ViaProxy backend) listening on {listen} -> {target} (version {})",
            version.viaproxy_name()
        );
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    } else {
        let proxy = ViaProxy::start(&listen, target.clone(), version).expect("bind Rust proxy");
        println!(
            "ViaHorizon (Rust minimal backend) listening on {} -> {} (protocol {})",
            proxy.listen_addr, target, version
        );
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
}
