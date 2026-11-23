//! Minimal multi-threaded TCP translating proxy bridging Horizon (777) <-> target server.

use crate::codec::{read_packet, write_packet};
use crate::pipeline::{Direction, Packet, ProtocolPipeline};
use crate::versions::ProtocolVersion;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

pub struct ViaProxy {
    pub listen_addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl ViaProxy {
    /// Starts a local translating proxy listening on `listen` and forwarding to `target`
    /// using `target_version`.
    pub fn start(listen: &str, target: String, target_version: ProtocolVersion) -> io::Result<Self> {
        let listener = TcpListener::bind(listen)?;
        let listen_addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = stop.clone();

        thread::spawn(move || {
            for incoming in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(client_stream) = incoming else {
                    continue;
                };
                let target_addr = target.clone();
                thread::spawn(move || {
                    let _ = handle_connection(client_stream, &target_addr, target_version);
                });
            }
        });

        Ok(Self { listen_addr, stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle_connection(
    client: TcpStream,
    target_addr: &str,
    target_version: ProtocolVersion,
) -> io::Result<()> {
    let server = TcpStream::connect(target_addr)?;
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);

    let pipeline = Arc::new(Mutex::new(ProtocolPipeline::new(target_version)));
    let pipe_c2s = pipeline.clone();

    let mut client_r = client.try_clone()?;
    let mut server_w = server.try_clone()?;
    let _c2s = thread::spawn(move || -> io::Result<()> {
        loop {
            let comp = pipe_c2s.lock().unwrap().compression;
            let (id, payload) = read_packet(&mut client_r, comp)?;
            let (out, next_comp) = {
                let mut p = pipe_c2s.lock().unwrap();
                let out = p.translate(Direction::Serverbound, Packet { id, payload })?;
                (out, p.compression)
            };
            for pkt in out {
                write_packet(&mut server_w, pkt.id, &pkt.payload, next_comp)?;
            }
        }
    });

    let mut server_r = server;
    let mut client_w = client;
    loop {
        let comp = pipeline.lock().unwrap().compression;
        let (id, payload) = read_packet(&mut server_r, comp)?;
        let (out, next_comp) = {
            let mut p = pipeline.lock().unwrap();
            let out = p.translate(Direction::Clientbound, Packet { id, payload })?;
            (out, p.compression)
        };
        for pkt in out {
            write_packet(&mut client_w, pkt.id, &pkt.payload, next_comp)?;
        }
    }
    #[allow(unreachable_code)]
    {
        let _ = c2s.join();
        Ok(())
    }
}
