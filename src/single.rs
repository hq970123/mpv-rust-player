//! Single instance: a second launch (double-clicking another file) hands its
//! file to the running window over a loopback socket and exits.
//!
//! The first instance listens on 127.0.0.1:PORT. A new instance tries to
//! connect; if a OK Player answers the handshake, it sends the path and
//! quits. Anything else on the port (or no answer) means we just run normally.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const PORT: u16 = 47391;
const HELLO: &str = "OK-PLAYER/1";

fn addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], PORT))
}

/// Try to hand `file` to an already running instance. Returns true when it
/// was accepted, in which case this process should exit.
pub fn forward(file: Option<&str>) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&addr(), Duration::from_millis(300)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(800)));
    let path = file
        .map(|f| {
            let p = PathBuf::from(f);
            if f.contains("://") {
                f.to_string()
            } else {
                std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().to_string()
            }
        })
        .unwrap_or_default();
    if writeln!(s, "{HELLO}\n{path}").is_err() {
        return false;
    }
    let mut line = String::new();
    BufReader::new(&s).read_line(&mut line).is_ok() && line.trim() == "OK"
}

/// Messages from later launches: Some(path) to open, None to just focus.
pub fn listen(ctx: eframe::egui::Context) -> Option<Receiver<Option<PathBuf>>> {
    let listener = TcpListener::bind(addr()).ok()?;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = conn.set_read_timeout(Some(Duration::from_millis(800)));
                let mut r = BufReader::new(&conn);
                let mut hello = String::new();
                if r.read_line(&mut hello).is_err() || hello.trim() != HELLO {
                    continue;
                }
                let mut path = String::new();
                let _ = r.read_line(&mut path);
                let path = path.trim().to_string();
                let _ = (&conn).write_all(b"OK\n");
                let msg = (!path.is_empty()).then(|| PathBuf::from(path));
                if tx.send(msg).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .ok()?;
    Some(rx)
}
