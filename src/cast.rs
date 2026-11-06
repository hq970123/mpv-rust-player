//! Casting to TVs and boxes over DLNA / UPnP AV.
//!
//!   discovery  — SSDP M-SEARCH for MediaRenderer devices, then each device's
//!                description XML for its AVTransport / RenderingControl URLs,
//!   media      — a tiny HTTP server on this machine streams the open file
//!                (HTTP Range, DLNA headers) and an optional SRT subtitle,
//!   control    — SOAP calls (SetAVTransportURI, Play, Pause, Seek, Stop,
//!                SetVolume) and a 1 s GetPositionInfo poll.
//!
//! The TV decodes the file itself, so formats it cannot play will fail there;
//! there is no transcoding in this version.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SSDP_ADDR: &str = "239.255.255.250:1900";
const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RC: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
const DLNA_FLAGS: &str = "DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000";

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub model: String,
    pub location: String,
    pub avt_url: String,
    pub rc_url: Option<String>,
}

// ------------------------------------------------------------------ discovery

#[derive(Default)]
pub struct Discovery {
    pub devices: Mutex<Vec<Device>>,
    pub searching: AtomicBool,
}

impl Discovery {
    /// Search the LAN for ~3 s on a background thread; results stream into `devices`.
    pub fn search(self: &Arc<Self>, ctx: eframe::egui::Context) {
        if self.searching.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut seen: Vec<String> = me.devices.lock().unwrap().iter().map(|d| d.location.clone()).collect();
            let found = ssdp_search(Duration::from_millis(3000));
            for loc in found {
                if seen.contains(&loc) {
                    continue;
                }
                seen.push(loc.clone());
                if let Some(d) = describe(&loc) {
                    let mut list = me.devices.lock().unwrap();
                    if !list.iter().any(|x| x.avt_url == d.avt_url) {
                        list.push(d);
                        list.sort_by(|a, b| a.name.cmp(&b.name));
                    }
                    ctx.request_repaint();
                }
            }
            me.searching.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }
}

fn ssdp_search(window: Duration) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out };
    let _ = sock.set_multicast_ttl_v4(4);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    for st in ["urn:schemas-upnp-org:device:MediaRenderer:1", AVT] {
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\nUSER-AGENT: FilmoraPlayer/1.0 UPnP/1.1\r\n\r\n"
        );
        for _ in 0..2 {
            let _ = sock.send_to(msg.as_bytes(), SSDP_ADDR);
        }
    }
    let start = Instant::now();
    let mut buf = [0u8; 2048];
    while start.elapsed() < window {
        if let Ok((n, _)) = sock.recv_from(&mut buf) {
            let text = String::from_utf8_lossy(&buf[..n]);
            if let Some(loc) = header(&text, "location") {
                if !out.contains(&loc) {
                    out.push(loc);
                }
            }
        }
    }
    out
}

fn header(msg: &str, name: &str) -> Option<String> {
    msg.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().to_string())
    })
}

fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(4))
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn describe(location: &str) -> Option<Device> {
    let xml = http().get(location).send().ok()?.text().ok()?;
    let base = tag(&xml, "URLBase").unwrap_or_else(|| origin(location));
    let mut avt = None;
    let mut rc = None;
    for svc in blocks(&xml, "service") {
        let ty = tag(&svc, "serviceType").unwrap_or_default();
        let ctl = tag(&svc, "controlURL").unwrap_or_default();
        if ty.contains("AVTransport") {
            avt = Some(join_url(&base, location, &ctl));
        } else if ty.contains("RenderingControl") {
            rc = Some(join_url(&base, location, &ctl));
        }
    }
    Some(Device {
        name: unescape(&tag(&xml, "friendlyName").unwrap_or_else(|| "DLNA".into())),
        model: unescape(&tag(&xml, "modelName").unwrap_or_default()),
        location: location.to_string(),
        avt_url: avt?,
        rc_url: rc,
    })
}

fn origin(url: &str) -> String {
    let after = url.find("://").map(|i| i + 3).unwrap_or(0);
    match url[after..].find('/') {
        Some(i) => url[..after + i].to_string(),
        None => url.to_string(),
    }
}

fn join_url(base: &str, location: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if path.starts_with('/') {
        return format!("{}{}", origin(base.trim_end_matches('/')), path);
    }
    let b = if base.ends_with('/') {
        base.to_string()
    } else if base == origin(location) {
        format!("{base}/")
    } else {
        base.rsplit_once('/').map(|(a, _)| format!("{a}/")).unwrap_or_else(|| format!("{base}/"))
    };
    format!("{b}{path}")
}

/// Text of the first `<name>` element (namespace prefixes ignored).
fn tag(xml: &str, name: &str) -> Option<String> {
    let mut rest = xml;
    loop {
        let i = rest.find('<')?;
        rest = &rest[i + 1..];
        let end = rest.find('>')?;
        let head = &rest[..end];
        let local = head.split_whitespace().next().unwrap_or("");
        let local = local.rsplit(':').next().unwrap_or(local);
        if local == name && !head.ends_with('/') {
            let body = &rest[end + 1..];
            let close = body.find("</")?;
            return Some(body[..close].trim().to_string());
        }
        rest = &rest[end..];
    }
}

fn blocks(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(after[..j].to_string());
        rest = &after[j + close.len()..];
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

// ------------------------------------------------------------------ media server

/// What the HTTP server hands out. Swapped when a new cast starts.
#[derive(Default)]
pub struct Served {
    pub media: Option<PathBuf>,
    pub token: String,
    /// Produces the SRT text on request (so live subtitles are as fresh as possible).
    pub subtitle: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

pub struct MediaServer {
    pub port: u16,
    pub served: Arc<Mutex<Served>>,
}

impl MediaServer {
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let port = listener.local_addr()?.port();
        let served: Arc<Mutex<Served>> = Arc::default();
        let s = served.clone();
        std::thread::Builder::new().name("cast-http".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let s = s.clone();
                std::thread::spawn(move || {
                    let _ = serve(conn, &s);
                });
            }
        })?;
        Ok(Self { port, served })
    }
}

pub fn mime_for(path: &std::path::Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" | "divx" => "video/x-msvideo",
        "ts" | "m2ts" | "mts" => "video/vnd.dlna.mpeg-tts",
        "mpg" | "mpeg" | "vob" => "video/mpeg",
        "wmv" | "asf" => "video/x-ms-wmv",
        "flv" | "f4v" => "video/x-flv",
        "3gp" | "3g2" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "video/mp4",
    }
}

fn serve(mut conn: TcpStream, served: &Arc<Mutex<Served>>) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let head_only = method.eq_ignore_ascii_case("HEAD");

    let (media, token, sub) = {
        let s = served.lock().unwrap();
        (s.media.clone(), s.token.clone(), s.subtitle.clone())
    };
    let host = headers.get("host").cloned().unwrap_or_default();

    if path.starts_with(&format!("/sub/{token}")) {
        let Some(f) = sub else { return not_found(&mut conn) };
        let body = f().into_bytes();
        let h = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/srt; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\ntransferMode.dlna.org: Interactive\r\nConnection: close\r\n\r\n",
            body.len()
        );
        conn.write_all(h.as_bytes())?;
        if !head_only {
            conn.write_all(&body)?;
        }
        return Ok(());
    }
    if !path.starts_with(&format!("/media/{token}")) || token.is_empty() {
        return not_found(&mut conn);
    }
    let Some(media) = media else { return not_found(&mut conn) };
    let mut file = std::fs::File::open(&media)?;
    let size = file.metadata()?.len();
    let (mut start, mut end) = (0u64, size.saturating_sub(1));
    let mut partial = false;
    if let Some(r) = headers.get("range").and_then(|r| r.strip_prefix("bytes=")) {
        let r = r.split(',').next().unwrap_or("");
        if let Some((a, b)) = r.split_once('-') {
            match (a.trim().parse::<u64>().ok(), b.trim().parse::<u64>().ok()) {
                (Some(a), Some(b)) => (start, end) = (a, b.min(end)),
                (Some(a), None) => start = a,
                (None, Some(n)) => start = size.saturating_sub(n),
                _ => {}
            }
            partial = true;
        }
    }
    if start > end || start >= size {
        let h = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nConnection: close\r\n\r\n");
        return conn.write_all(h.as_bytes());
    }
    let len = end - start + 1;
    let mime = mime_for(&media);
    let mut h = String::new();
    h.push_str(if partial { "HTTP/1.1 206 Partial Content\r\n" } else { "HTTP/1.1 200 OK\r\n" });
    h.push_str(&format!("Content-Type: {mime}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\n"));
    if partial {
        h.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
    }
    h.push_str(&format!("transferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {DLNA_FLAGS}\r\n"));
    if sub.is_some() && !host.is_empty() {
        // Samsung TVs pick the subtitle up from this header.
        h.push_str(&format!("CaptionInfo.sec: http://{host}/sub/{token}.srt\r\n"));
    }
    h.push_str("Connection: close\r\n\r\n");
    conn.write_all(h.as_bytes())?;
    if head_only {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut left = len;
    let mut buf = vec![0u8; 256 * 1024];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        conn.write_all(&buf[..n])?;
        left -= n as u64;
    }
    Ok(())
}

fn not_found(conn: &mut TcpStream) -> std::io::Result<()> {
    conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

/// The address of this machine as seen from `device_url`'s host.
fn local_ip_towards(device_url: &str) -> Option<IpAddr> {
    let host_port = origin(device_url);
    let hp = host_port.split("://").nth(1)?;
    let addr: SocketAddr = if hp.contains(':') {
        hp.parse().ok()?
    } else {
        format!("{hp}:80").parse().ok()?
    };
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(addr).ok()?;
    Some(s.local_addr().ok()?.ip())
}

// ------------------------------------------------------------------ SOAP

fn soap(url: &str, service: &str, action: &str, args: &str) -> Result<String, String> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
<s:Body><u:{action} xmlns:u=\"{service}\">{args}</u:{action}></s:Body></s:Envelope>"
    );
    let resp = http()
        .post(url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{service}#{action}\""))
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    let ok = resp.status().is_success();
    let text = resp.text().unwrap_or_default();
    if ok {
        Ok(text)
    } else {
        let detail = tag(&text, "errorDescription").or_else(|| tag(&text, "faultstring")).unwrap_or_default();
        let code = tag(&text, "errorCode").unwrap_or_default();
        Err(format!("{action} failed {code} {detail}").trim().to_string())
    }
}

fn hms(t: f64) -> String {
    let s = t.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn parse_hms(s: &str) -> Option<f64> {
    let mut total = 0.0;
    for p in s.trim().split(':') {
        total = total * 60.0 + p.parse::<f64>().ok()?;
    }
    Some(total)
}

fn didl(title: &str, media_url: &str, mime: &str, size: u64, sub_url: Option<&str>) -> String {
    let class = if mime.starts_with("audio") { "object.item.audioItem.musicTrack" } else { "object.item.videoItem" };
    let mut s = String::from(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" xmlns:sec=\"http://www.sec.co.kr/\" xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\">",
    );
    s.push_str("<item id=\"0\" parentID=\"-1\" restricted=\"1\">");
    s.push_str(&format!("<dc:title>{}</dc:title><upnp:class>{class}</upnp:class>", escape(title)));
    s.push_str(&format!(
        "<res protocolInfo=\"http-get:*:{mime}:{DLNA_FLAGS}\" size=\"{size}\">{}</res>",
        escape(media_url)
    ));
    if let Some(u) = sub_url {
        s.push_str(&format!("<res protocolInfo=\"http-get:*:text/srt:*\">{}</res>", escape(u)));
        s.push_str(&format!("<sec:CaptionInfoEx sec:type=\"srt\">{}</sec:CaptionInfoEx>", escape(u)));
        s.push_str(&format!("<sec:CaptionInfo sec:type=\"srt\">{}</sec:CaptionInfo>", escape(u)));
    }
    s.push_str("</item></DIDL-Lite>");
    s
}

// ------------------------------------------------------------------ session

#[derive(Debug, Clone, Default, PartialEq)]
pub enum CastState {
    #[default]
    Idle,
    Connecting,
    Playing,
    Paused,
    Buffering,
    Stopped,
    Error(String),
}

#[derive(Default)]
pub struct CastStatus {
    pub state: Mutex<CastState>,
    pub device: Mutex<Option<Device>>,
    pos: AtomicU64,
    dur: AtomicU64,
    pub volume: AtomicU64,
}

impl CastStatus {
    pub fn pos(&self) -> f64 {
        f64::from_bits(self.pos.load(Ordering::Relaxed))
    }
    pub fn duration(&self) -> f64 {
        f64::from_bits(self.dur.load(Ordering::Relaxed))
    }
    pub fn state(&self) -> CastState {
        self.state.lock().unwrap().clone()
    }
    fn set_state(&self, s: CastState) {
        *self.state.lock().unwrap() = s;
    }
}

enum Cmd {
    Start { device: Device, media: PathBuf, title: String, start: f64, has_sub: bool },
    Play,
    Pause,
    Seek(f64),
    Volume(u32),
    Stop,
}

pub struct Caster {
    tx: Sender<Cmd>,
    pub status: Arc<CastStatus>,
    pub server: Option<Arc<MediaServer>>,
    pub discovery: Arc<Discovery>,
}

impl Caster {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        let (tx, rx) = channel();
        let status: Arc<CastStatus> = Arc::default();
        status.volume.store(30f64.to_bits(), Ordering::Relaxed);
        let server = MediaServer::start().ok().map(Arc::new);
        let (st, sv) = (status.clone(), server.clone());
        std::thread::Builder::new()
            .name("cast".into())
            .spawn(move || session(rx, st, sv, ctx))
            .expect("spawn cast worker");
        Self { tx, status, server, discovery: Arc::default() }
    }

    /// Start casting `media`. `subtitle` is called whenever the TV fetches the SRT.
    pub fn start(
        &self,
        device: Device,
        media: PathBuf,
        start: f64,
        subtitle: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    ) {
        let title = media.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let has_sub = subtitle.is_some();
        if let Some(s) = &self.server {
            let mut sv = s.served.lock().unwrap();
            sv.media = Some(media.clone());
            sv.token = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
            sv.subtitle = subtitle;
        }
        let _ = self.tx.send(Cmd::Start { device, media, title, start, has_sub });
    }
    pub fn play(&self) {
        let _ = self.tx.send(Cmd::Play);
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    pub fn seek(&self, t: f64) {
        self.status.pos.store(t.to_bits(), Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Seek(t));
    }
    pub fn set_volume(&self, v: f64) {
        self.status.volume.store(v.to_bits(), Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Volume(v.clamp(0.0, 100.0) as u32));
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

fn session(rx: Receiver<Cmd>, st: Arc<CastStatus>, server: Option<Arc<MediaServer>>, ctx: eframe::egui::Context) {
    let mut dev: Option<Device> = None;
    let mut pending_seek: Option<(f64, Instant)> = None;
    let mut last_poll = Instant::now();
    let mut seek_guard = Instant::now();
    loop {
        let cmd = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let avt = |d: &Device, action: &str, args: &str| soap(&d.avt_url, AVT, action, args);
        match cmd {
            Some(Cmd::Start { device, media, title, start, has_sub }) => {
                if let Some(old) = dev.take() {
                    if old.avt_url != device.avt_url {
                        let _ = avt(&old, "Stop", "<InstanceID>0</InstanceID>");
                    }
                }
                st.set_state(CastState::Connecting);
                *st.device.lock().unwrap() = Some(device.clone());
                st.pos.store(start.to_bits(), Ordering::Relaxed);
                ctx.request_repaint();
                let Some(server) = &server else {
                    st.set_state(CastState::Error("media server unavailable".into()));
                    continue;
                };
                let Some(ip) = local_ip_towards(&device.avt_url) else {
                    st.set_state(CastState::Error("no network route to device".into()));
                    continue;
                };
                let token = server.served.lock().unwrap().token.clone();
                let ext = media.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
                let host = match ip {
                    IpAddr::V4(v) => format!("{v}:{}", server.port),
                    IpAddr::V6(v) => format!("[{v}]:{}", server.port),
                };
                let url = format!("http://{host}/media/{token}.{ext}");
                let sub_url = has_sub.then(|| format!("http://{host}/sub/{token}.srt"));
                let size = std::fs::metadata(&media).map(|m| m.len()).unwrap_or(0);
                let meta = didl(&title, &url, mime_for(&media), size, sub_url.as_deref());
                let _ = avt(&device, "Stop", "<InstanceID>0</InstanceID>");
                let args = format!(
                    "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>",
                    escape(&url),
                    escape(&meta)
                );
                let res = avt(&device, "SetAVTransportURI", &args)
                    .or_else(|_| {
                        // Some renderers reject rich metadata; retry without it.
                        let args = format!(
                            "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData></CurrentURIMetaData>",
                            escape(&url)
                        );
                        avt(&device, "SetAVTransportURI", &args)
                    })
                    .and_then(|_| avt(&device, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>"));
                match res {
                    Ok(_) => {
                        st.set_state(CastState::Buffering);
                        if start > 2.0 {
                            pending_seek = Some((start, Instant::now()));
                        }
                        if let Some(rc) = &device.rc_url {
                            if let Ok(x) = soap(rc, RC, "GetVolume", "<InstanceID>0</InstanceID><Channel>Master</Channel>") {
                                if let Some(v) = tag(&x, "CurrentVolume").and_then(|v| v.parse::<f64>().ok()) {
                                    st.volume.store(v.to_bits(), Ordering::Relaxed);
                                }
                            }
                        }
                        dev = Some(device);
                    }
                    Err(e) => st.set_state(CastState::Error(e)),
                }
            }
            Some(Cmd::Play) => {
                if let Some(d) = &dev {
                    if avt(d, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>").is_ok() {
                        st.set_state(CastState::Playing);
                    }
                }
            }
            Some(Cmd::Pause) => {
                if let Some(d) = &dev {
                    if avt(d, "Pause", "<InstanceID>0</InstanceID>").is_ok() {
                        st.set_state(CastState::Paused);
                    }
                }
            }
            Some(Cmd::Seek(t)) => {
                if let Some(d) = &dev {
                    let _ = avt(d, "Seek", &format!("<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>{}</Target>", hms(t)));
                    seek_guard = Instant::now();
                }
            }
            Some(Cmd::Volume(v)) => {
                if let Some(rc) = dev.as_ref().and_then(|d| d.rc_url.clone()) {
                    let _ = soap(
                        &rc,
                        RC,
                        "SetVolume",
                        &format!("<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>{v}</DesiredVolume>"),
                    );
                }
            }
            Some(Cmd::Stop) => {
                if let Some(d) = dev.take() {
                    let _ = avt(&d, "Stop", "<InstanceID>0</InstanceID>");
                }
                pending_seek = None;
                st.set_state(CastState::Stopped);
                if let Some(s) = &server {
                    let mut sv = s.served.lock().unwrap();
                    sv.media = None;
                    sv.subtitle = None;
                }
                ctx.request_repaint();
            }
            None => {}
        }

        // poll position / transport state once a second
        let Some(d) = dev.clone() else { continue };
        if last_poll.elapsed() < Duration::from_millis(1000) {
            continue;
        }
        last_poll = Instant::now();
        if let Ok(x) = avt(&d, "GetTransportInfo", "<InstanceID>0</InstanceID>") {
            let s = tag(&x, "CurrentTransportState").unwrap_or_default();
            let new = match s.as_str() {
                "PLAYING" => Some(CastState::Playing),
                "PAUSED_PLAYBACK" | "PAUSED" => Some(CastState::Paused),
                "TRANSITIONING" => Some(CastState::Buffering),
                "STOPPED" | "NO_MEDIA_PRESENT" => {
                    // Treat as finished only once it has actually played.
                    matches!(st.state(), CastState::Playing | CastState::Paused).then_some(CastState::Stopped)
                }
                _ => None,
            };
            if let Some(n) = new {
                if n == CastState::Stopped {
                    dev = None;
                }
                st.set_state(n);
            }
        }
        if seek_guard.elapsed() > Duration::from_millis(1500) {
            if let Ok(x) = avt(&d, "GetPositionInfo", "<InstanceID>0</InstanceID>") {
                if let Some(p) = tag(&x, "RelTime").and_then(|s| parse_hms(&s)) {
                    st.pos.store(p.to_bits(), Ordering::Relaxed);
                }
                if let Some(t) = tag(&x, "TrackDuration").and_then(|s| parse_hms(&s)).filter(|t| *t > 0.0) {
                    st.dur.store(t.to_bits(), Ordering::Relaxed);
                }
            }
        }
        if let Some((t, at)) = pending_seek {
            if st.state() == CastState::Playing || at.elapsed() > Duration::from_secs(8) {
                pending_seek = None;
                let _ = avt(&d, "Seek", &format!("<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>{}</Target>", hms(t)));
                seek_guard = Instant::now();
                st.pos.store(t.to_bits(), Ordering::Relaxed);
            }
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_description() {
        let xml = r#"<root><URLBase>http://192.168.1.5:49152/</URLBase><device><friendlyName>Living Room TV</friendlyName>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/upnp/control/AVTransport1</controlURL></service>
<service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>upnp/control/RenderingControl1</controlURL></service></serviceList></device></root>"#;
        assert_eq!(tag(xml, "friendlyName").as_deref(), Some("Living Room TV"));
        let svcs = blocks(xml, "service");
        assert_eq!(svcs.len(), 2);
        let base = tag(xml, "URLBase").unwrap();
        let loc = "http://192.168.1.5:49152/description.xml";
        assert_eq!(join_url(&base, loc, "/upnp/control/AVTransport1"), "http://192.168.1.5:49152/upnp/control/AVTransport1");
        assert_eq!(join_url(&base, loc, "upnp/control/RenderingControl1"), "http://192.168.1.5:49152/upnp/control/RenderingControl1");
        let base2 = origin(loc);
        assert_eq!(join_url(&base2, loc, "ctl"), "http://192.168.1.5:49152/ctl");
    }

    #[test]
    fn times() {
        assert_eq!(hms(3725.4), "01:02:05");
        assert_eq!(parse_hms("01:02:05.500"), Some(3725.5));
    }

    #[test]
    fn serves_ranges() {
        let dir = std::env::temp_dir().join("fp-cast-test");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("clip.mp4");
        std::fs::write(&f, (0..=255u8).collect::<Vec<_>>()).unwrap();
        let srv = MediaServer::start().unwrap();
        {
            let mut s = srv.served.lock().unwrap();
            s.media = Some(f);
            s.token = "abc".into();
            s.subtitle = Some(Arc::new(|| "1\n00:00:00,000 --> 00:00:01,000\nhi\n\n".to_string()));
        }
        let c = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
        let r = c
            .get(format!("http://127.0.0.1:{}/media/abc.mp4", srv.port))
            .header("Range", "bytes=10-19")
            .send()
            .unwrap();
        assert_eq!(r.status().as_u16(), 206);
        assert!(r.headers().get("CaptionInfo.sec").is_some());
        assert_eq!(r.bytes().unwrap().to_vec(), (10..20u8).collect::<Vec<_>>());
        let r = c.get(format!("http://127.0.0.1:{}/sub/abc.srt", srv.port)).send().unwrap();
        assert!(r.text().unwrap().contains("hi"));
        let r = c.get(format!("http://127.0.0.1:{}/media/zzz.mp4", srv.port)).send().unwrap();
        assert_eq!(r.status().as_u16(), 404);
    }
}
