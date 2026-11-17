//! Casting to TVs and boxes over DLNA / UPnP AV, and to Google Cast devices
//! (see `chromecast`; they share the media server and the session loop).
//!
//!   discovery  — SSDP M-SEARCH for MediaRenderer devices, then each device's
//!                description XML for its AVTransport / RenderingControl URLs,
//!   media      — a tiny HTTP server on this machine streams the open file
//!                (HTTP Range, DLNA headers) and an optional SRT subtitle,
//!   control    — SOAP calls (SetAVTransportURI, Play, Pause, Seek, Stop,
//!                SetVolume) and a 1 s GetPositionInfo poll.
//!
//! The TV decodes the file itself unless the file is transcoded first
//! (see `transcode`): then the server streams a growing MPEG-TS file, and
//! seeking restarts the encoder at the new position.

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
/// Live transcoded stream: no byte or time seeking, converted content.
const LIVE_FLAGS: &str = "DLNA.ORG_OP=00;DLNA.ORG_CI=1;DLNA.ORG_FLAGS=01700000000000000000000000000000";
const LIVE_MIME: &str = "video/mpeg";

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub model: String,
    pub location: String,
    pub avt_url: String,
    pub rc_url: Option<String>,
    /// Set for a Google Cast device (its CASTV2 address); DLNA otherwise.
    pub chromecast: Option<SocketAddr>,
}

impl Device {
    pub fn is_chromecast(&self) -> bool {
        self.chromecast.is_some()
    }
}

// ------------------------------------------------------------------ discovery

#[derive(Default)]
pub struct Discovery {
    pub devices: Mutex<Vec<Device>>,
    pub searching: AtomicBool,
}

impl Discovery {
    fn add(&self, d: Device, ctx: &eframe::egui::Context) {
        let mut list = self.devices.lock().unwrap();
        if !list.iter().any(|x| x.location == d.location || (!d.avt_url.is_empty() && x.avt_url == d.avt_url)) {
            list.push(d);
            list.sort_by(|a, b| a.name.cmp(&b.name));
        }
        ctx.request_repaint();
    }

    /// Search the LAN for ~3 s on background threads (SSDP for DLNA, mDNS for
    /// Google Cast); results stream into `devices`.
    pub fn search(self: &Arc<Self>, ctx: eframe::egui::Context) {
        if self.searching.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let (me2, ctx2) = (self.clone(), ctx.clone());
        let mdns = std::thread::spawn(move || {
            crate::chromecast::discover(Duration::from_millis(3000), |f| {
                let d = Device {
                    name: f.name,
                    model: f.model,
                    location: format!("cast://{}", f.addr),
                    avt_url: String::new(),
                    rc_url: None,
                    chromecast: Some(f.addr),
                };
                me2.add(d, &ctx2);
            });
        });
        std::thread::spawn(move || {
            let mut seen: Vec<String> = me.devices.lock().unwrap().iter().map(|d| d.location.clone()).collect();
            let found = ssdp_search(Duration::from_millis(3000));
            for loc in found {
                if seen.contains(&loc) {
                    continue;
                }
                seen.push(loc.clone());
                if let Some(d) = describe(&loc) {
                    me.add(d, &ctx);
                }
            }
            let _ = mdns.join();
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
            "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\nUSER-AGENT: OKPlayer/1.0 UPnP/1.1\r\n\r\n"
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
        chromecast: None,
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
    /// Set for a file that is still being written by the transcoder; the
    /// flag turns true once it is complete.
    pub live: Option<Arc<AtomicBool>>,
    /// Highest byte offset handed out (throttles the transcoder).
    pub sent: Arc<AtomicU64>,
    /// Seconds the served stream starts into the source; subtitles shift by it.
    pub sub_offset: f64,
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
    if method.eq_ignore_ascii_case("OPTIONS") {
        // CORS preflight from the Chromecast receiver page.
        return conn.write_all(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    }

    let (media, token, sub, live, sent, sub_offset) = {
        let s = served.lock().unwrap();
        (s.media.clone(), s.token.clone(), s.subtitle.clone(), s.live.clone(), s.sent.clone(), s.sub_offset)
    };
    let host = headers.get("host").cloned().unwrap_or_default();

    if path.starts_with(&format!("/sub/{token}")) {
        let Some(f) = sub else { return not_found(&mut conn) };
        let srt = crate::transcode::shift_srt(&f(), sub_offset);
        let (body, ctype) = if path.ends_with(".vtt") {
            (crate::transcode::srt_to_vtt(&srt).into_bytes(), "text/vtt")
        } else {
            (srt.into_bytes(), "text/srt")
        };
        let h = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\ntransferMode.dlna.org: Interactive\r\nConnection: close\r\n\r\n",
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
    if let Some(done) = live {
        return serve_growing(conn, &media, &done, &sent, served, &token, head_only, sub.is_some(), &host);
    }
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
        let h =
            format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nConnection: close\r\n\r\n");
        return conn.write_all(h.as_bytes());
    }
    let len = end - start + 1;
    let mime = mime_for(&media);
    let mut h = String::new();
    h.push_str(if partial { "HTTP/1.1 206 Partial Content\r\n" } else { "HTTP/1.1 200 OK\r\n" });
    h.push_str(&format!(
        "Content-Type: {mime}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\n"
    ));
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

/// Stream a file the transcoder is still writing: no length, no ranges,
/// follow the tail until the encoder is done or a new cast replaces it.
#[allow(clippy::too_many_arguments)]
fn serve_growing(
    mut conn: TcpStream,
    media: &std::path::Path,
    done: &AtomicBool,
    sent: &AtomicU64,
    served: &Arc<Mutex<Served>>,
    token: &str,
    head_only: bool,
    has_sub: bool,
    host: &str,
) -> std::io::Result<()> {
    let mime = if media.extension().is_some_and(|e| e == "mp4") { "video/mp4" } else { LIVE_MIME };
    let mut h = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nAccess-Control-Allow-Origin: *\r\ntransferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {LIVE_FLAGS}\r\n"
    );
    if has_sub && !host.is_empty() {
        h.push_str(&format!("CaptionInfo.sec: http://{host}/sub/{token}.srt\r\n"));
    }
    h.push_str("Connection: close\r\n\r\n");
    conn.write_all(h.as_bytes())?;
    if head_only {
        return Ok(());
    }
    let t0 = Instant::now();
    let mut file = loop {
        match std::fs::File::open(media) {
            Ok(f) => break f,
            Err(e) if t0.elapsed() > Duration::from_secs(20) || done.load(Ordering::SeqCst) => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let mut pos = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    let mut idle = Instant::now();
    loop {
        let n = file.read(&mut buf)?;
        if n > 0 {
            conn.write_all(&buf[..n])?;
            pos += n as u64;
            sent.fetch_max(pos, Ordering::Relaxed);
            idle = Instant::now();
            continue;
        }
        if done.load(Ordering::SeqCst) {
            // One more read: the encoder may have flushed after our last one.
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            conn.write_all(&buf[..n])?;
            continue;
        }
        if served.lock().unwrap().token != token || idle.elapsed() > Duration::from_secs(60) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
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
    let addr: SocketAddr = if hp.contains(':') { hp.parse().ok()? } else { format!("{hp}:80").parse().ok()? };
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

fn didl(title: &str, media_url: &str, mime: &str, flags: &str, size: u64, sub_url: Option<&str>) -> String {
    let class = if mime.starts_with("audio") { "object.item.audioItem.musicTrack" } else { "object.item.videoItem" };
    let mut s = String::from(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" xmlns:sec=\"http://www.sec.co.kr/\" xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\">",
    );
    s.push_str("<item id=\"0\" parentID=\"-1\" restricted=\"1\">");
    s.push_str(&format!("<dc:title>{}</dc:title><upnp:class>{class}</upnp:class>", escape(title)));
    let size = if size > 0 { format!(" size=\"{size}\"") } else { String::new() };
    s.push_str(&format!("<res protocolInfo=\"http-get:*:{mime}:{flags}\"{size}>{}</res>", escape(media_url)));
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
    /// Waiting for the transcoder's first megabyte.
    Transcoding,
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
    /// The current cast goes through the transcoder.
    pub transcoding: AtomicBool,
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

#[derive(Clone)]
struct StartArgs {
    device: Device,
    media: PathBuf,
    title: String,
    has_sub: bool,
    transcode: Option<crate::transcode::Job>,
}

enum Cmd {
    Start(StartArgs, f64),
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
        crate::transcode::clean_temp();
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
    /// With `transcode`, the file is converted on the fly (its `start` is ignored).
    pub fn start(
        &self,
        device: Device,
        media: PathBuf,
        start: f64,
        subtitle: Option<Arc<dyn Fn() -> String + Send + Sync>>,
        transcode: Option<crate::transcode::Job>,
    ) {
        let title = media.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let has_sub = subtitle.is_some();
        if let Some(s) = &self.server {
            s.served.lock().unwrap().subtitle = subtitle;
        }
        self.status.transcoding.store(transcode.is_some(), Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Start(StartArgs { device, media, title, has_sub, transcode }, start));
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

fn new_token() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos())
}

fn avt(d: &Device, action: &str, args: &str) -> Result<String, String> {
    soap(&d.avt_url, AVT, action, args)
}

/// The device a session is talking to.
enum Remote {
    Dlna(Device),
    Cast(Device, Box<crate::chromecast::Conn>),
}

/// One poll's findings, whatever the protocol.
#[derive(Default)]
struct Polled {
    state: Option<CastState>,
    pos: Option<f64>,
    dur: Option<f64>,
    /// 0..100
    volume: Option<f64>,
}

impl Remote {
    fn device(&self) -> &Device {
        match self {
            Remote::Dlna(d) | Remote::Cast(d, _) => d,
        }
    }
    fn play(&mut self) -> Result<(), String> {
        match self {
            Remote::Dlna(d) => avt(d, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>").map(|_| ()),
            Remote::Cast(_, c) => c.play(),
        }
    }
    fn pause(&mut self) -> Result<(), String> {
        match self {
            Remote::Dlna(d) => avt(d, "Pause", "<InstanceID>0</InstanceID>").map(|_| ()),
            Remote::Cast(_, c) => c.pause(),
        }
    }
    fn seek(&mut self, t: f64) {
        match self {
            Remote::Dlna(d) => {
                let _ = avt(
                    d,
                    "Seek",
                    &format!("<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>{}</Target>", hms(t)),
                );
            }
            Remote::Cast(_, c) => {
                let _ = c.seek(t);
            }
        }
    }
    /// `v` 0..100
    fn volume(&mut self, v: u32) {
        match self {
            Remote::Dlna(d) => {
                if let Some(rc) = &d.rc_url {
                    let _ = soap(
                        rc,
                        RC,
                        "SetVolume",
                        &format!(
                            "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>{v}</DesiredVolume>"
                        ),
                    );
                }
            }
            Remote::Cast(_, c) => {
                let _ = c.set_volume(v as f64 / 100.0);
            }
        }
    }
    fn stop(&mut self) {
        match self {
            Remote::Dlna(d) => {
                let _ = avt(d, "Stop", "<InstanceID>0</InstanceID>");
            }
            Remote::Cast(_, c) => c.stop(),
        }
    }
    fn current_volume(&mut self) -> Option<f64> {
        match self {
            Remote::Dlna(d) => {
                let rc = d.rc_url.as_ref()?;
                let x = soap(rc, RC, "GetVolume", "<InstanceID>0</InstanceID><Channel>Master</Channel>").ok()?;
                tag(&x, "CurrentVolume").and_then(|v| v.parse::<f64>().ok())
            }
            Remote::Cast(_, c) => c.volume.map(|l| l * 100.0),
        }
    }
    /// `was`: the state before this poll (a stop only counts once something played).
    fn poll(&mut self, was: &CastState, want_pos: bool) -> Polled {
        let mut p = Polled::default();
        let played = matches!(was, CastState::Playing | CastState::Paused);
        match self {
            Remote::Dlna(d) => {
                if let Ok(x) = avt(d, "GetTransportInfo", "<InstanceID>0</InstanceID>") {
                    p.state = match tag(&x, "CurrentTransportState").unwrap_or_default().as_str() {
                        "PLAYING" => Some(CastState::Playing),
                        "PAUSED_PLAYBACK" | "PAUSED" => Some(CastState::Paused),
                        "TRANSITIONING" => Some(CastState::Buffering),
                        "STOPPED" | "NO_MEDIA_PRESENT" => played.then_some(CastState::Stopped),
                        _ => None,
                    };
                }
                if want_pos {
                    if let Ok(x) = avt(d, "GetPositionInfo", "<InstanceID>0</InstanceID>") {
                        p.pos = tag(&x, "RelTime").and_then(|s| parse_hms(&s));
                        p.dur = tag(&x, "TrackDuration").and_then(|s| parse_hms(&s)).filter(|t| *t > 0.0);
                    }
                }
            }
            Remote::Cast(_, c) => match c.poll() {
                Ok(s) => {
                    use crate::chromecast::Player;
                    p.state = match s.state {
                        Some(Player::Playing) => Some(CastState::Playing),
                        Some(Player::Paused) => Some(CastState::Paused),
                        Some(Player::Buffering) => Some(CastState::Buffering),
                        Some(Player::Idle(r)) if r == "ERROR" => {
                            Some(CastState::Error("Cast: playback error on the device".into()))
                        }
                        Some(Player::Idle(r)) if r == "FINISHED" || played => Some(CastState::Stopped),
                        _ => None,
                    };
                    if let Some(e) = s.error {
                        p.state = Some(CastState::Error(e));
                    } else if s.closed {
                        p.state = Some(CastState::Stopped);
                    }
                    if want_pos {
                        p.pos = s.pos;
                    }
                    p.dur = s.dur;
                    p.volume = s.volume.map(|l| l * 100.0);
                }
                Err(e) => p.state = Some(CastState::Error(e)),
            },
        }
        p
    }
}

/// Content type for the Default Media Receiver (it checks it with canPlayType).
fn cast_mime(path: &std::path::Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "video/mp4",
    }
}

/// Point the server at the media (starting the transcoder if needed) and tell
/// the device to play it from `start`. `prev` is reused when it is a Cast
/// connection to the same device.
fn begin(
    a: &StartArgs,
    start: f64,
    server: &MediaServer,
    st: &CastStatus,
    ctx: &eframe::egui::Context,
    prev: Option<Remote>,
) -> Result<(Remote, Option<crate::transcode::Transcoder>), String> {
    let toward = match a.device.chromecast {
        Some(addr) => format!("http://{addr}"),
        None => a.device.avt_url.clone(),
    };
    let ip = local_ip_towards(&toward).ok_or("no network route to device")?;
    let token = new_token();
    let host = match ip {
        IpAddr::V4(v) => format!("{v}:{}", server.port),
        IpAddr::V6(v) => format!("[{v}]:{}", server.port),
    };
    let mut tc = None;
    let (url, mime, flags, size) = if let Some(job) = &a.transcode {
        let mut job = job.clone();
        job.start = start;
        let ext = if job.fmp4 { "mp4" } else { "ts" };
        job.out = crate::transcode::temp_dir().join(format!("{token}.{ext}"));
        let sent = Arc::new(AtomicU64::new(0));
        let t = crate::transcode::Transcoder::start(job, sent.clone());
        {
            let mut sv = server.served.lock().unwrap();
            sv.media = Some(t.out.clone());
            sv.token = token.clone();
            sv.live = Some(t.done.clone());
            sv.sent = sent;
            sv.sub_offset = start;
        }
        st.set_state(CastState::Transcoding);
        ctx.request_repaint();
        t.wait_ready(Duration::from_secs(45))?;
        tc = Some(t);
        let mime = if ext == "mp4" { "video/mp4" } else { LIVE_MIME };
        (format!("http://{host}/media/{token}.{ext}"), mime, LIVE_FLAGS, 0)
    } else {
        {
            let mut sv = server.served.lock().unwrap();
            sv.media = Some(a.media.clone());
            sv.token = token.clone();
            sv.live = None;
            sv.sub_offset = 0.0;
        }
        let ext = a.media.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_else(|| "mp4".into());
        let size = std::fs::metadata(&a.media).map(|m| m.len()).unwrap_or(0);
        (format!("http://{host}/media/{token}.{ext}"), mime_for(&a.media), DLNA_FLAGS, size)
    };

    if let Some(addr) = a.device.chromecast {
        let mut conn = match prev {
            Some(Remote::Cast(d, c)) if d.chromecast == Some(addr) => c,
            _ => {
                let mut c = crate::chromecast::Conn::connect(addr)?;
                c.launch()?;
                Box::new(c)
            }
        };
        let sub_url = a.has_sub.then(|| format!("http://{host}/sub/{token}.vtt"));
        let mime = if tc.is_some() { "video/mp4" } else { cast_mime(&a.media) };
        let from = if tc.is_some() { 0.0 } else { start };
        if let Err(e) = conn.load(&url, mime, &a.title, from, sub_url.as_deref()) {
            // The receiver app may have been closed on the TV since: relaunch once.
            if conn.launch().is_err() {
                return Err(e);
            }
            conn.load(&url, mime, &a.title, from, sub_url.as_deref())?;
        }
        st.set_state(CastState::Buffering);
        return Ok((Remote::Cast(a.device.clone(), conn), tc));
    }

    if let Some(mut p) = prev {
        if p.device().location != a.device.location {
            p.stop();
        }
    }
    let sub_url = a.has_sub.then(|| format!("http://{host}/sub/{token}.srt"));
    let meta = didl(&a.title, &url, mime, flags, size, sub_url.as_deref());
    let _ = avt(&a.device, "Stop", "<InstanceID>0</InstanceID>");
    let args = format!(
        "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>",
        escape(&url),
        escape(&meta)
    );
    avt(&a.device, "SetAVTransportURI", &args)
        .or_else(|_| {
            // Some renderers reject rich metadata; retry without it.
            let args = format!(
                "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData></CurrentURIMetaData>",
                escape(&url)
            );
            avt(&a.device, "SetAVTransportURI", &args)
        })
        .and_then(|_| avt(&a.device, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>"))?;
    st.set_state(CastState::Buffering);
    Ok((Remote::Dlna(a.device.clone()), tc))
}

fn session(rx: Receiver<Cmd>, st: Arc<CastStatus>, server: Option<Arc<MediaServer>>, ctx: eframe::egui::Context) {
    let mut dev: Option<Remote> = None;
    let mut args: Option<StartArgs> = None;
    let mut tc: Option<crate::transcode::Transcoder> = None;
    // Source time at which the served stream starts (transcoding only).
    let mut offset = 0.0f64;
    let mut pending_seek: Option<(f64, Instant)> = None;
    let mut last_poll = Instant::now();
    let mut seek_guard = Instant::now();
    let mut queued: std::collections::VecDeque<Cmd> = Default::default();
    loop {
        let next = match queued.pop_front() {
            Some(c) => Ok(c),
            None => rx.recv_timeout(Duration::from_millis(250)),
        };
        let cmd = match next {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut d) = dev.take() {
                    d.stop();
                }
                return;
            }
        };
        // Start, or restart a transcoded stream at a new position.
        let mut launch: Option<(StartArgs, f64)> = None;
        match cmd {
            Some(Cmd::Start(a, start)) => {
                if let Some(mut old) = dev.take() {
                    if old.device().location != a.device.location {
                        old.stop();
                    } else {
                        dev = Some(old); // reused by `begin`
                    }
                }
                st.set_state(CastState::Connecting);
                *st.device.lock().unwrap() = Some(a.device.clone());
                st.dur.store(0f64.to_bits(), Ordering::Relaxed);
                launch = Some((a, start));
            }
            Some(Cmd::Play) => {
                if let Some(d) = dev.as_mut() {
                    if d.play().is_ok() {
                        st.set_state(CastState::Playing);
                    }
                }
            }
            Some(Cmd::Pause) => {
                if let Some(d) = dev.as_mut() {
                    if d.pause().is_ok() {
                        st.set_state(CastState::Paused);
                    }
                }
            }
            Some(Cmd::Seek(mut t)) => {
                if tc.is_some() {
                    // Restarting the encoder is costly: only the last of a burst of seeks counts.
                    std::thread::sleep(Duration::from_millis(150));
                    while let Ok(c) = rx.try_recv() {
                        match c {
                            Cmd::Seek(x) => t = x,
                            other => queued.push_back(other),
                        }
                    }
                    if let Some(a) = args.clone() {
                        launch = Some((a, t));
                    }
                } else if let Some(d) = dev.as_mut() {
                    d.seek(t);
                    seek_guard = Instant::now();
                }
            }
            Some(Cmd::Volume(v)) => {
                if let Some(d) = dev.as_mut() {
                    d.volume(v);
                }
            }
            Some(Cmd::Stop) => {
                if let Some(mut d) = dev.take() {
                    d.stop();
                }
                tc = None;
                args = None;
                pending_seek = None;
                st.set_state(CastState::Stopped);
                if let Some(s) = &server {
                    let mut sv = s.served.lock().unwrap();
                    sv.media = None;
                    sv.subtitle = None;
                    sv.live = None;
                    sv.token = new_token();
                }
                ctx.request_repaint();
            }
            None => {}
        }

        if let Some((a, start)) = launch {
            tc = None; // stops the previous encoder
            pending_seek = None;
            st.pos.store(start.to_bits(), Ordering::Relaxed);
            ctx.request_repaint();
            let Some(server) = &server else {
                st.set_state(CastState::Error("media server unavailable".into()));
                continue;
            };
            match begin(&a, start, server, &st, &ctx, dev.take()) {
                Ok((mut r, t)) => {
                    offset = if t.is_some() { start } else { 0.0 };
                    if t.is_none() && start > 2.0 && matches!(r, Remote::Dlna(_)) {
                        pending_seek = Some((start, Instant::now()));
                    }
                    tc = t;
                    if let Some(v) = r.current_volume() {
                        st.volume.store(v.to_bits(), Ordering::Relaxed);
                    }
                    dev = Some(r);
                    args = Some(a);
                    seek_guard = Instant::now();
                }
                Err(e) => {
                    dev = None;
                    args = None;
                    st.set_state(CastState::Error(e));
                }
            }
            ctx.request_repaint();
        }

        // poll position / transport state once a second
        let Some(d) = dev.as_mut() else { continue };
        if last_poll.elapsed() < Duration::from_millis(1000) {
            continue;
        }
        last_poll = Instant::now();
        if let Some(e) = tc.as_ref().and_then(|t| t.error.lock().unwrap().clone()) {
            d.stop();
            dev = None;
            tc = None;
            st.set_state(CastState::Error(e));
            ctx.request_repaint();
            continue;
        }
        let p = d.poll(&st.state(), seek_guard.elapsed() > Duration::from_millis(1500));
        if let Some(n) = p.state {
            if matches!(n, CastState::Stopped | CastState::Error(_)) {
                dev = None;
                tc = None;
            }
            st.set_state(n);
        }
        if let Some(pos) = p.pos {
            st.pos.store((offset + pos).to_bits(), Ordering::Relaxed);
        }
        // A growing stream has no meaningful duration; the app keeps the local one.
        if let (Some(t), true) = (p.dur, tc.is_none()) {
            st.dur.store(t.to_bits(), Ordering::Relaxed);
        }
        if let Some(v) = p.volume {
            st.volume.store(v.to_bits(), Ordering::Relaxed);
        }
        if let (Some((t, at)), Some(d)) = (pending_seek, dev.as_mut()) {
            if st.state() == CastState::Playing || at.elapsed() > Duration::from_secs(8) {
                pending_seek = None;
                d.seek(t);
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
        assert_eq!(
            join_url(&base, loc, "/upnp/control/AVTransport1"),
            "http://192.168.1.5:49152/upnp/control/AVTransport1"
        );
        assert_eq!(
            join_url(&base, loc, "upnp/control/RenderingControl1"),
            "http://192.168.1.5:49152/upnp/control/RenderingControl1"
        );
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
