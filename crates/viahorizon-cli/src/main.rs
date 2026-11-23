//! Standalone ViaHorizon launcher: auto-manages mini JRE + official Java ViaProxy
//! (ViaVersion + ViaBackwards + ViaRewind) for Minecraft 1.7.10 -> 26.3.

use std::path::PathBuf;
use viahorizon_core::{ProtocolVersion, ViaJavaProxy};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut listen = "127.0.0.1:25566".to_owned();
    let mut target = None::<String>;
    let mut version = ProtocolVersion::V1_8_9;
    let mut data_dir = PathBuf::from(".viahorizon");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data-dir" if i + 1 < args.len() => {
                data_dir = PathBuf::from(&args[i + 1]);
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
            "Usage: viahorizon-cli [--data-dir <dir>] --listen 127.0.0.1:25566 \
             --target <host:port> --version <1.7.10..26.3>"
        );
        std::process::exit(2);
    };

    let _proxy = ViaJavaProxy::start_auto(&data_dir, Some(&listen), &target, version)
        .expect("start ViaProxy");
    println!(
        "ViaHorizon (ViaProxy) listening on {listen} -> {target} (version {})",
        version.viaproxy_name()
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
