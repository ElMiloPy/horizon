//! Minimal standalone ViaHorizon proxy CLI.
//!
//! Usage:
//!   viahorizon-cli --listen 127.0.0.1:25566 --target play.example.net:25565 --version 1.20.4

use viahorizon_core::{ProtocolVersion, ViaProxy};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut listen = "127.0.0.1:25566".to_owned();
    let mut target = None::<String>;
    let mut version = ProtocolVersion::V1_20_4;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
        eprintln!("Usage: viahorizon-cli --listen 127.0.0.1:25566 --target <host:port> --version <1.8.9..26.3>");
        std::process::exit(2);
    };

    let proxy = ViaProxy::start(&listen, target.clone(), version).expect("bind proxy");
    println!("ViaHorizon listening on {} -> {} (protocol {})", proxy.listen_addr, target, version);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
