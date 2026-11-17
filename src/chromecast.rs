//! Google Cast (Chromecast, Google TV, Android TVs with Cast, Nest Hub).
//!
//!   discovery — mDNS query for `_googlecast._tcp.local` (legacy unicast, so
//!               no need to own port 5353), TXT `fn` / `md` give name and model,
//!   control   — CASTV2: length-prefixed protobuf `CastMessage`s over TLS on
//!               port 8009 (the device certificate is self-signed), JSON
//!               payloads; launches the Default Media Receiver and LOADs the
//!               URL served by `cast::MediaServer`.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NS_CONN: &str = "urn:x-cast:com.google.cast.tp.connection";
const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
/// Google's Default Media Receiver.
const DEFAULT_RECEIVER: &str = "CC1AD845";
const SENDER: &str = "sender-okplayer";
const PLATFORM: &str = "receiver-0";
const SERVICE: &str = "_googlecast._tcp.local";

// ------------------------------------------------------------------ discovery

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    pub model: String,
    pub addr: SocketAddr,
    pub id: String,
}

/// Ask the LAN for Cast devices for `window`; each new one goes to `on_found`.
pub fn discover(window: Duration, mut on_found: impl FnMut(Found)) {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return };
    let _ = sock.set_multicast_ttl_v4(255);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
    let q = query(SERVICE);
    let dest = SocketAddr::from(([224, 0, 0, 251], 5353));
    let start = Instant::now();
    let mut next_send = start;
    let mut seen: Vec<Found> = Vec::new();
    let mut buf = [0u8; 9000];
    while start.elapsed() < window {
        if Instant::now() >= next_send {
            let _ = sock.send_to(&q, dest);
            next_send += Duration::from_millis(1000);
        }
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            for f in parse_response(&buf[..n], from.ip()) {
                if !seen.iter().any(|o| o.id == f.id || o.addr == f.addr) {
                    seen.push(f.clone());
                    on_found(f);
                }
            }
        }
    }
}

/// A PTR question with the unicast-response bit set.
fn query(name: &str) -> Vec<u8> {
    let mut p = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&[0, 12, 0x80, 0x01]);
    p
}

fn read_name(msg: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *msg.get(pos + 1)? as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            jumps += 1;
            if jumps > 32 {
                return None;
            }
            pos = ptr;
            continue;
        }
        labels.push(String::from_utf8_lossy(msg.get(pos + 1..pos + 1 + len)?).into_owned());
        pos += 1 + len;
    }
    Some((labels.join("."), end.unwrap_or(pos)))
}

fn parse_response(msg: &[u8], from: IpAddr) -> Vec<Found> {
    let rd = |i: usize| -> Option<u16> { Some(u16::from_be_bytes([*msg.get(i)?, *msg.get(i + 1)?])) };
    let mut out = Vec::new();
    let (Some(flags), Some(qd), Some(an), Some(ns), Some(ar)) = (rd(2), rd(4), rd(6), rd(8), rd(10)) else {
        return out;
    };
    if flags & 0x8000 == 0 {
        return out; // a query, not an answer
    }
    let mut pos = 12;
    for _ in 0..qd {
        let Some((_, p)) = read_name(msg, pos) else { return out };
        pos = p + 4;
    }
    let mut ptrs: Vec<String> = Vec::new();
    let mut srv: HashMap<String, (u16, String)> = HashMap::new();
    let mut txt: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut a: HashMap<String, Ipv4Addr> = HashMap::new();
    for _ in 0..(an as usize + ns as usize + ar as usize) {
        let Some((name, p)) = read_name(msg, pos) else { break };
        let (Some(ty), Some(len)) = (rd(p), rd(p + 8)) else { break };
        let data = p + 10;
        let end = data + len as usize;
        if end > msg.len() {
            break;
        }
        let key = name.to_ascii_lowercase();
        match ty {
            12 => {
                if let Some((t, _)) = read_name(msg, data) {
                    ptrs.push(t);
                }
            }
            33 => {
                if let (Some(port), Some((target, _))) = (rd(data + 4), read_name(msg, data + 6)) {
                    srv.insert(key, (port, target.to_ascii_lowercase()));
                }
            }
            16 => {
                let mut kv = HashMap::new();
                let mut i = data;
                while i < end {
                    let l = msg[i] as usize;
                    let s = String::from_utf8_lossy(&msg[(i + 1).min(end)..(i + 1 + l).min(end)]).into_owned();
                    if let Some((k, v)) = s.split_once('=') {
                        kv.insert(k.to_ascii_lowercase(), v.to_string());
                    }
                    i += 1 + l;
                }
                txt.insert(key, kv);
            }
            1 if len == 4 => {
                a.insert(key, Ipv4Addr::new(msg[data], msg[data + 1], msg[data + 2], msg[data + 3]));
            }
            _ => {}
        }
        pos = end;
    }
    let mut instances: Vec<String> = ptrs.iter().map(|p| p.to_ascii_lowercase()).collect();
    for k in srv.keys().chain(txt.keys()) {
        if !instances.contains(k) {
            instances.push(k.clone());
        }
    }
    let original: HashMap<String, String> = ptrs.iter().map(|p| (p.to_ascii_lowercase(), p.clone())).collect();
    for key in instances {
        if !key.ends_with(&format!(".{SERVICE}")) {
            continue;
        }
        let (port, target) = srv.get(&key).cloned().unwrap_or((8009, String::new()));
        let ip = a.get(&target).map(|v| IpAddr::V4(*v)).unwrap_or(from);
        let t = txt.get(&key);
        let label = original.get(&key).cloned().unwrap_or_else(|| key.clone());
        let name = t
            .and_then(|t| t.get("fn"))
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| label.split("._googlecast").next().unwrap_or("Chromecast").to_string());
        let model = t.and_then(|t| t.get("md")).cloned().unwrap_or_default();
        let id = t.and_then(|t| t.get("id")).cloned().unwrap_or(key);
        out.push(Found { name, model, addr: SocketAddr::new(ip, port), id });
    }
    out
}

// ------------------------------------------------------------------ wire format

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn string_field(out: &mut Vec<u8>, n: u64, s: &str) {
    varint(out, (n << 3) | 2);
    varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

/// A framed `CastMessage` (protocol CASTV2_1_0, string payload).
fn encode(source: &str, dest: &str, namespace: &str, payload: &str) -> Vec<u8> {
    let mut m = Vec::new();
    varint(&mut m, 1 << 3);
    varint(&mut m, 0);
    string_field(&mut m, 2, source);
    string_field(&mut m, 3, dest);
    string_field(&mut m, 4, namespace);
    varint(&mut m, 5 << 3);
    varint(&mut m, 0);
    string_field(&mut m, 6, payload);
    let mut f = (m.len() as u32).to_be_bytes().to_vec();
    f.extend(m);
    f
}

#[derive(Debug, Default, PartialEq)]
struct Msg {
    source: String,
    dest: String,
    namespace: String,
    payload: String,
}

fn read_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let x = *b.get(*i)?;
        *i += 1;
        v |= ((x & 0x7f) as u64) << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn decode(b: &[u8]) -> Option<Msg> {
    let mut i = 0;
    let mut m = Msg::default();
    while i < b.len() {
        let key = read_varint(b, &mut i)?;
        match key & 7 {
            0 => {
                read_varint(b, &mut i)?;
            }
            2 => {
                let l = read_varint(b, &mut i)? as usize;
                let s = String::from_utf8_lossy(b.get(i..i + l)?).into_owned();
                i += l;
                match key >> 3 {
                    2 => m.source = s,
                    3 => m.dest = s,
                    4 => m.namespace = s,
                    6 => m.payload = s,
                    _ => {}
                }
            }
            1 => i += 8,
            5 => i += 4,
            _ => return None,
        }
    }
    Some(m)
}

// ------------------------------------------------------------------ TLS

/// Cast devices present a certificate chained to Google's device CA that no
/// system trust store knows; senders accept it as is.
#[derive(Debug)]
struct AnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

// ------------------------------------------------------------------ session

#[derive(Debug, Clone, PartialEq)]
pub enum Player {
    Playing,
    Paused,
    Buffering,
    /// Idle with the receiver's reason (FINISHED, CANCELLED, ERROR, ...).
    Idle(String),
}

/// What one poll learned; `None` fields were not reported.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub state: Option<Player>,
    pub pos: Option<f64>,
    pub dur: Option<f64>,
    /// 0..1
    pub volume: Option<f64>,
    /// The receiver app went away (closed on the TV, or another sender took over).
    pub closed: bool,
    pub error: Option<String>,
}

pub struct Conn {
    tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    buf: Vec<u8>,
    req: i64,
    transport: String,
    session: String,
    media: Option<i64>,
    last_ping: Instant,
    pub volume: Option<f64>,
}

type Res<T> = Result<T, String>;

fn find_app(v: &Value) -> Option<&Value> {
    v["status"]["applications"].as_array()?.iter().find(|a| a["appId"] == DEFAULT_RECEIVER)
}

impl Conn {
    pub fn connect(addr: SocketAddr) -> Res<Self> {
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| format!("{addr}: {e}"))?;
        let _ = tcp.set_nodelay(true);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::IpAddress(addr.ip().into());
        let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| e.to_string())?;
        let mut c = Self {
            tls: rustls::StreamOwned::new(conn, tcp),
            buf: Vec::new(),
            req: 0,
            transport: String::new(),
            session: String::new(),
            media: None,
            last_ping: Instant::now(),
            volume: None,
        };
        c.send(NS_CONN, PLATFORM, json!({"type": "CONNECT", "origin": {}}))?;
        Ok(c)
    }

    fn send(&mut self, ns: &str, dest: &str, v: Value) -> Res<()> {
        let frame = encode(SENDER, dest, ns, &v.to_string());
        self.tls.write_all(&frame).and_then(|_| self.tls.flush()).map_err(|e| format!("Cast: {e}"))
    }

    fn request(&mut self, ns: &str, dest: &str, mut v: Value) -> Res<i64> {
        self.req += 1;
        v["requestId"] = json!(self.req);
        self.send(ns, dest, v)?;
        Ok(self.req)
    }

    /// Messages arriving within `wait` (returns as soon as some are in).
    /// Heartbeat PINGs are answered here.
    fn recv(&mut self, wait: Duration) -> Res<Vec<(String, String, Value)>> {
        let deadline = Instant::now() + wait;
        let mut out = Vec::new();
        let mut tmp = [0u8; 16384];
        loop {
            while self.buf.len() >= 4 {
                let n = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
                if n > 1 << 20 {
                    return Err("Cast: bad frame".into());
                }
                if self.buf.len() < 4 + n {
                    break;
                }
                let frame: Vec<u8> = self.buf.drain(..4 + n).skip(4).collect();
                let Some(m) = decode(&frame) else { continue };
                let v: Value = serde_json::from_str(&m.payload).unwrap_or(Value::Null);
                if m.namespace == NS_HEARTBEAT {
                    if v["type"] == "PING" {
                        self.send(NS_HEARTBEAT, &m.source, json!({"type": "PONG"}))?;
                    }
                    continue;
                }
                out.push((m.namespace, m.source, v));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if !out.is_empty() || left.is_zero() {
                break;
            }
            let _ = self.tls.sock.set_read_timeout(Some(left.max(Duration::from_millis(10))));
            match self.tls.read(&mut tmp) {
                Ok(0) => return Err("Cast: the device closed the connection".into()),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(format!("Cast: {e}")),
            }
        }
        Ok(out)
    }

    fn observe(&mut self, ns: &str, src: &str, v: &Value, st: &mut Status) {
        let ty = v["type"].as_str().unwrap_or("");
        match (ns, ty) {
            (NS_RECEIVER, "RECEIVER_STATUS") => {
                if let Some(l) = v["status"]["volume"]["level"].as_f64() {
                    st.volume = Some(l);
                    self.volume = Some(l);
                }
                if !self.transport.is_empty()
                    && find_app(v).map(|a| a["transportId"] != self.transport.as_str()).unwrap_or(true)
                {
                    st.closed = true;
                }
            }
            (NS_CONN, "CLOSE") if src == self.transport => st.closed = true,
            (NS_MEDIA, "MEDIA_STATUS") => match v["status"].as_array() {
                Some(list) if !list.is_empty() => {
                    let s = &list[0];
                    if let Some(id) = s["mediaSessionId"].as_i64() {
                        self.media = Some(id);
                    }
                    st.pos = s["currentTime"].as_f64();
                    if let Some(d) = s["media"]["duration"].as_f64().filter(|d| *d > 0.0) {
                        st.dur = Some(d);
                    }
                    st.state = match s["playerState"].as_str() {
                        Some("PLAYING") => Some(Player::Playing),
                        Some("PAUSED") => Some(Player::Paused),
                        Some("BUFFERING") | Some("LOADING") => Some(Player::Buffering),
                        Some("IDLE") => Some(Player::Idle(s["idleReason"].as_str().unwrap_or("").to_string())),
                        _ => None,
                    };
                }
                Some(_) if self.media.is_some() => st.state = Some(Player::Idle(String::new())),
                _ => {}
            },
            (NS_MEDIA, "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "ERROR") => {
                st.error =
                    Some(format!("Cast: {}", v["reason"].as_str().or(v["detailedErrorCode"].as_str()).unwrap_or(ty)));
            }
            _ => {}
        }
    }

    fn wait_for(&mut self, timeout: Duration, mut f: impl FnMut(&str, &Value) -> Option<Res<Value>>) -> Res<Value> {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            for (ns, src, v) in self.recv(Duration::from_millis(250))? {
                let mut scratch = Status::default();
                self.observe(&ns, &src, &v, &mut scratch);
                if let Some(r) = f(&ns, &v) {
                    return r;
                }
            }
        }
        Err("Cast: the device did not answer".into())
    }

    /// Start (or join) the Default Media Receiver on the device.
    pub fn launch(&mut self) -> Res<()> {
        self.request(NS_RECEIVER, PLATFORM, json!({"type": "GET_STATUS"}))?;
        let status_of =
            |ns: &str, v: &Value| (ns == NS_RECEIVER && v["type"] == "RECEIVER_STATUS").then(|| Ok(v.clone()));
        let mut status = self.wait_for(Duration::from_secs(6), status_of)?;
        if find_app(&status).is_none() {
            self.request(NS_RECEIVER, PLATFORM, json!({"type": "LAUNCH", "appId": DEFAULT_RECEIVER}))?;
            status = self.wait_for(Duration::from_secs(20), |ns, v| {
                if ns != NS_RECEIVER {
                    return None;
                }
                match v["type"].as_str() {
                    Some("RECEIVER_STATUS") if find_app(v).is_some() => Some(Ok(v.clone())),
                    Some("LAUNCH_ERROR") => {
                        Some(Err(format!("Cast: launch failed ({})", v["reason"].as_str().unwrap_or("?"))))
                    }
                    _ => None,
                }
            })?;
        }
        let app = find_app(&status).ok_or("Cast: receiver app missing")?;
        self.transport = app["transportId"].as_str().unwrap_or_default().to_string();
        self.session = app["sessionId"].as_str().unwrap_or_default().to_string();
        if let Some(l) = status["status"]["volume"]["level"].as_f64() {
            self.volume = Some(l);
        }
        self.media = None;
        let t = self.transport.clone();
        self.send(NS_CONN, &t, json!({"type": "CONNECT", "origin": {}}))
    }

    /// Play `url` from `start`; `sub` is a WebVTT URL shown as subtitles.
    pub fn load(&mut self, url: &str, mime: &str, title: &str, start: f64, sub: Option<&str>) -> Res<()> {
        let mut msg = json!({
            "type": "LOAD",
            "sessionId": self.session,
            "autoplay": true,
            "currentTime": start.max(0.0),
            "media": {
                "contentId": url,
                "contentUrl": url,
                "contentType": mime,
                "streamType": "BUFFERED",
                "metadata": {"metadataType": 0, "title": title},
            },
        });
        if let Some(u) = sub {
            msg["media"]["tracks"] = json!([{
                "trackId": 1, "type": "TEXT", "subtype": "SUBTITLES",
                "trackContentId": u, "trackContentType": "text/vtt", "name": "Subtitles", "language": "und",
            }]);
            msg["media"]["textTrackStyle"] = json!({
                "backgroundColor": "#00000000", "foregroundColor": "#FFFFFFFF",
                "edgeType": "OUTLINE", "edgeColor": "#000000FF", "fontScale": 1.0,
            });
            msg["activeTrackIds"] = json!([1]);
        }
        let t = self.transport.clone();
        let id = self.request(NS_MEDIA, &t, msg)?;
        self.media = None;
        self.wait_for(Duration::from_secs(40), |ns, v| {
            if ns != NS_MEDIA {
                return None;
            }
            let mine = v["requestId"].as_i64() == Some(id);
            match v["type"].as_str().unwrap_or("") {
                "MEDIA_STATUS" if mine || v["status"][0]["media"]["contentId"] == url => Some(Ok(v.clone())),
                "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST" | "ERROR" if mine => Some(Err(format!(
                    "Cast: the device could not play this file ({})",
                    v["reason"].as_str().or(v["detailedErrorCode"].as_str()).unwrap_or("LOAD_FAILED")
                ))),
                _ => None,
            }
        })?;
        if self.media.is_none() {
            return Err("Cast: no media session".into());
        }
        Ok(())
    }

    fn media_cmd(&mut self, ty: &str, extra: Value) -> Res<()> {
        let Some(m) = self.media else { return Err("Cast: nothing is loaded".into()) };
        let mut v = json!({"type": ty, "mediaSessionId": m});
        if let Value::Object(e) = extra {
            for (k, x) in e {
                v[k.as_str()] = x;
            }
        }
        let t = self.transport.clone();
        self.request(NS_MEDIA, &t, v).map(|_| ())
    }

    pub fn play(&mut self) -> Res<()> {
        self.media_cmd("PLAY", Value::Null)
    }
    pub fn pause(&mut self) -> Res<()> {
        self.media_cmd("PAUSE", Value::Null)
    }
    pub fn seek(&mut self, t: f64) -> Res<()> {
        self.media_cmd("SEEK", json!({"currentTime": t.max(0.0)}))
    }
    /// `level` 0..1
    pub fn set_volume(&mut self, level: f64) -> Res<()> {
        self.request(NS_RECEIVER, PLATFORM, json!({"type": "SET_VOLUME", "volume": {"level": level.clamp(0.0, 1.0)}}))
            .map(|_| ())
    }

    /// Stop playback and close the receiver app on the TV.
    pub fn stop(&mut self) {
        let _ = self.media_cmd("STOP", Value::Null);
        if !self.session.is_empty() {
            let s = self.session.clone();
            let _ = self.request(NS_RECEIVER, PLATFORM, json!({"type": "STOP", "sessionId": s}));
        }
        let t = self.transport.clone();
        if !t.is_empty() {
            let _ = self.send(NS_CONN, &t, json!({"type": "CLOSE"}));
        }
        self.media = None;
        self.transport.clear();
        self.session.clear();
    }

    /// Ask for the media status, keep the connection alive, and report.
    pub fn poll(&mut self) -> Res<Status> {
        if self.last_ping.elapsed() > Duration::from_secs(5) {
            self.send(NS_HEARTBEAT, PLATFORM, json!({"type": "PING"}))?;
            self.last_ping = Instant::now();
        }
        if let Some(m) = self.media {
            let t = self.transport.clone();
            self.request(NS_MEDIA, &t, json!({"type": "GET_STATUS", "mediaSessionId": m}))?;
        }
        let mut st = Status::default();
        let end = Instant::now() + Duration::from_millis(500);
        while Instant::now() < end && st.state.is_none() {
            let left = end.saturating_duration_since(Instant::now());
            for (ns, src, v) in self.recv(left)? {
                self.observe(&ns, &src, &v, &mut st);
            }
        }
        Ok(st)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = encode("sender-0", "receiver-0", NS_RECEIVER, r#"{"type":"GET_STATUS"}"#);
        let n = u32::from_be_bytes([f[0], f[1], f[2], f[3]]) as usize;
        assert_eq!(n, f.len() - 4);
        let m = decode(&f[4..]).unwrap();
        assert_eq!(m.source, "sender-0");
        assert_eq!(m.dest, "receiver-0");
        assert_eq!(m.namespace, NS_RECEIVER);
        assert_eq!(m.payload, r#"{"type":"GET_STATUS"}"#);
        let mut long = Vec::new();
        varint(&mut long, 300);
        assert_eq!(long, vec![0xAC, 0x02]);
    }

    fn name(p: &mut Vec<u8>, n: &str) {
        for l in n.split('.') {
            p.push(l.len() as u8);
            p.extend_from_slice(l.as_bytes());
        }
        p.push(0);
    }

    fn rr(p: &mut Vec<u8>, owner_ptr: Option<u16>, owner: &str, ty: u16, data: &[u8]) {
        match owner_ptr {
            Some(o) => p.extend_from_slice(&(0xC000 | o).to_be_bytes()),
            None => name(p, owner),
        }
        p.extend_from_slice(&ty.to_be_bytes());
        p.extend_from_slice(&[0x80, 1, 0, 0, 0, 120]);
        p.extend_from_slice(&(data.len() as u16).to_be_bytes());
        p.extend_from_slice(data);
    }

    #[test]
    fn parses_mdns_answer() {
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3];
        let svc_at = p.len() as u16;
        let mut ptr = Vec::new();
        name(&mut ptr, "Chromecast-abc._googlecast._tcp.local");
        rr(&mut p, None, SERVICE, 12, &ptr);
        let inst = "Chromecast-abc._googlecast._tcp.local";
        let mut txt = Vec::new();
        for s in ["id=abc123", "md=Chromecast Ultra", "fn=Living Room"] {
            txt.push(s.len() as u8);
            txt.extend_from_slice(s.as_bytes());
        }
        rr(&mut p, None, inst, 16, &txt);
        let mut srv = vec![0, 0, 0, 0, 0x1f, 0x49];
        name(&mut srv, "abc.local");
        rr(&mut p, None, inst, 33, &srv);
        rr(&mut p, None, "abc.local", 1, &[192, 168, 1, 40]);
        let _ = svc_at;
        let found = parse_response(&p, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Living Room");
        assert_eq!(found[0].model, "Chromecast Ultra");
        assert_eq!(found[0].id, "abc123");
        assert_eq!(found[0].addr, "192.168.1.40:8009".parse().unwrap());
        // Our own query is not an answer.
        assert!(parse_response(&query(SERVICE), IpAddr::V4(Ipv4Addr::LOCALHOST)).is_empty());
    }
}
