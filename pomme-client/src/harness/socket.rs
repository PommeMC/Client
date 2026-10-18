//! The live control socket: one JSON [`Command`] per line in, one JSON reply
//! per line out, served on 127.0.0.1 only.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::{Value, json};

use super::protocol::Command;

/// Where a command's reply goes.
pub type Reply = Sender<Result<Value, String>>;
/// A command from the socket and where its reply goes.
pub type Request = (Command, Reply);

pub fn serve(port: u16) -> std::io::Result<Receiver<Request>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    tracing::info!("Test harness listening on 127.0.0.1:{port}");
    let (tx, rx) = unbounded();
    std::thread::Builder::new()
        .name("test-harness".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Err(e) = handle(stream, &tx) {
                    tracing::warn!("Test harness connection ended: {e}");
                }
            }
        })?;
    Ok(rx)
}

fn handle(stream: TcpStream, requests: &Sender<Request>) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Command>(&line) {
            Ok(command) => {
                let (reply_tx, reply_rx) = bounded(1);
                if requests.send((command, reply_tx)).is_err() {
                    return Ok(());
                }
                reply_rx
                    .recv()
                    .unwrap_or_else(|_| Err("client shut down".into()))
            }
            Err(e) => Err(e.to_string()),
        };
        let reply = match reply {
            Ok(data) => json!({ "ok": true, "data": data }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        writeln!(out, "{reply}")?;
    }
    Ok(())
}
