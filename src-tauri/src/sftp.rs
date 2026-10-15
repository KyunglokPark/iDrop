// 최소 SFTP v3 클라이언트 (draft-ietf-secsh-filexfer-02)
//
// SSH 세션 채널의 "sftp" 서브시스템 위에서 동작한다. 요청마다 id를 붙여 보내고
// 읽기 태스크가 응답을 id로 짝지어 돌려주므로, 여러 요청을 동시에 띄울 수 있다
// (다운로드/업로드는 32KB 조각을 여러 개 겹쳐 보내 지연이 큰 망에서도 빠르게).
// 파일 이름은 원시 바이트 — 서버 인코딩(UTF-8 / EUC-KR)에 맞춰 바꾼다.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_WRITE: u8 = 6;
const FXP_LSTAT: u8 = 7;
const FXP_OPENDIR: u8 = 11;
const FXP_READDIR: u8 = 12;
const FXP_REMOVE: u8 = 13;
const FXP_MKDIR: u8 = 14;
const FXP_RMDIR: u8 = 15;
const FXP_REALPATH: u8 = 16;
const FXP_STAT: u8 = 17;
const FXP_RENAME: u8 = 18;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_NAME: u8 = 104;
const FXP_ATTRS: u8 = 105;

const FX_OK: u32 = 0;
const FX_EOF: u32 = 1;

const FXF_READ: u32 = 0x01;
const FXF_WRITE: u32 = 0x02;
const FXF_CREAT: u32 = 0x08;
const FXF_TRUNC: u32 = 0x10;

const ATTR_SIZE: u32 = 0x01;
const ATTR_UIDGID: u32 = 0x02;
const ATTR_PERMISSIONS: u32 = 0x04;
const ATTR_ACMODTIME: u32 = 0x08;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// 한 번에 읽고 쓰는 조각 크기와 동시에 띄우는 요청 수
const CHUNK: u32 = 32 * 1024;
const WINDOW: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct Attrs {
    pub size: Option<u64>,
    pub perm: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.perm.map(|p| p & 0o170000 == 0o040000).unwrap_or(false)
    }
    pub fn is_link(&self) -> bool {
        self.perm.map(|p| p & 0o170000 == 0o120000).unwrap_or(false)
    }
}

/// 목록 항목 (프런트로 보낸다)
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: u32,
    pub perm: u32,
}

/// 응답 패킷 (타입 + id 뒤의 본문)
struct Resp {
    typ: u8,
    body: Vec<u8>,
}

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Resp>>>>;

/// 파일명 인코딩 상태 — 같은 서버의 **모든 SFTP 채널이 공유**한다 (Arc로 주입).
/// iDrop은 탐색 채널에서 나열하고 다른 전송 채널에서 그 파일을 열기 때문에,
/// 채널마다 따로 두면 EUC-KR 이름이 캐시 미스로 "no such file"이 된다. [iDOT 대비 변경]
pub struct NameCodec {
    /// 새 파일 이름을 EUC-KR로 만든다 (서버 로케일이 EUC-KR일 때)
    pub euc: AtomicBool,
    /// 목록에서 본 경로 → 서버의 원시 이름 바이트. 한 서버에 UTF-8 이름과 EUC-KR 이름이
    /// 섞여 있어도 기존 파일은 원래 바이트 그대로 가리키게 한다.
    raw_names: Mutex<HashMap<String, Vec<u8>>>,
}

impl Default for NameCodec {
    fn default() -> Self {
        NameCodec {
            euc: AtomicBool::new(false),
            raw_names: Mutex::new(HashMap::new()),
        }
    }
}

pub struct Sftp {
    out: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next_id: AtomicU32,
    /// 사이트 공유 파일명 인코딩 상태
    pub codec: Arc<NameCodec>,
    closed: Arc<AtomicBool>,
}

// ---- 바이트 다루기 ---------------------------------------------------------------

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}
fn put_u64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_be_bytes());
}
fn put_str(b: &mut Vec<u8>, s: &[u8]) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s);
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, p: 0 }
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.p..self.p + 4).ok_or("short packet")?;
        self.p += 4;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let hi = self.u32()? as u64;
        let lo = self.u32()? as u64;
        Ok(hi << 32 | lo)
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        let s = self.b.get(self.p..self.p + n).ok_or("short packet")?;
        self.p += n;
        Ok(s)
    }
    fn attrs(&mut self) -> Result<Attrs, String> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            self.u32()?;
            self.u32()?;
        }
        if flags & ATTR_PERMISSIONS != 0 {
            a.perm = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            self.u32()?; // atime
            a.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            let n = self.u32()?;
            for _ in 0..n {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(a)
    }
}

fn status_err(body: &[u8]) -> String {
    let mut r = Rd::new(body);
    let code = r.u32().unwrap_or(4);
    let msg = r.bytes().map(|m| String::from_utf8_lossy(m).into_owned()).unwrap_or_default();
    let what = match code {
        2 => "no such file",
        3 => "permission denied",
        4 => "failure",
        5 => "bad message",
        6 => "no connection",
        7 => "connection lost",
        8 => "operation unsupported",
        _ => "error",
    };
    if msg.is_empty() || msg.eq_ignore_ascii_case(what) {
        what.to_string()
    } else {
        msg
    }
}

impl Sftp {
    /// 서브시스템 스트림 위에서 SFTP를 시작한다 (INIT → VERSION).
    /// `codec`은 같은 사이트의 모든 채널이 공유하는 파일명 인코딩 상태.
    pub async fn start<S>(stream: S, codec: Arc<NameCodec>) -> Result<Arc<Sftp>, String>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (mut rd, mut wr) = tokio::io::split(stream);
        // INIT
        let mut init = Vec::new();
        put_u32(&mut init, 5);
        init.push(FXP_INIT);
        put_u32(&mut init, 3);
        wr.write_all(&init).await.map_err(|e| e.to_string())?;
        let first = read_packet(&mut rd).await.map_err(|e| e.to_string())?;
        if first.first() != Some(&FXP_VERSION) {
            return Err("SFTP server did not answer".into());
        }

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // 쓰기 태스크
        tokio::spawn(async move {
            while let Some(pkt) = rx.recv().await {
                if wr.write_all(&pkt).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });
        // 읽기 태스크: id로 응답을 돌려준다
        let pend = pending.clone();
        let cl = closed.clone();
        tokio::spawn(async move {
            while let Ok(p) = read_packet(&mut rd).await {
                if p.len() < 5 {
                    continue;
                }
                let id = u32::from_be_bytes([p[1], p[2], p[3], p[4]]);
                if let Some(tx) = pend.lock().unwrap().remove(&id) {
                    let _ = tx.send(Resp { typ: p[0], body: p[5..].to_vec() });
                }
            }
            cl.store(true, Ordering::SeqCst);
            pend.lock().unwrap().clear(); // 대기 중인 요청은 모두 실패
        });
        Ok(Arc::new(Sftp {
            out: tx,
            pending,
            next_id: AtomicU32::new(1),
            codec,
            closed,
        }))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.out.is_closed()
    }

    /// 요청을 보내고 응답을 기다릴 수신기를 돌려준다 (겹쳐 보내기용).
    fn send(&self, typ: u8, payload: &[u8]) -> Result<oneshot::Receiver<Resp>, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut pkt = Vec::with_capacity(payload.len() + 9);
        put_u32(&mut pkt, payload.len() as u32 + 5);
        pkt.push(typ);
        put_u32(&mut pkt, id);
        pkt.extend_from_slice(payload);
        self.out.send(pkt).map_err(|_| "SFTP connection closed".to_string())?;
        Ok(rx)
    }

    async fn call(&self, typ: u8, payload: &[u8]) -> Result<Resp, String> {
        self.send(typ, payload)?
            .await
            .map_err(|_| "SFTP connection closed".to_string())
    }

    async fn call_status(&self, typ: u8, payload: &[u8]) -> Result<(), String> {
        let r = self.call(typ, payload).await?;
        expect_ok(&r)
    }

    // ---- 이름 인코딩 ----

    /// 새 이름(글자) → 서버 인코딩 바이트
    fn enc_fresh(&self, s: &str) -> Vec<u8> {
        if self.codec.euc.load(Ordering::Relaxed) {
            encoding_rs::EUC_KR.encode(s).0.into_owned()
        } else {
            s.as_bytes().to_vec()
        }
    }

    /// 경로 → 바이트. 목록에서 본 경로면 서버의 원래 바이트를, 아니면 상위 폴더는
    /// 원래 바이트 + 마지막 이름만 새로 인코딩한다.
    pub fn enc(&self, path: &str) -> Vec<u8> {
        if let Some(b) = self.codec.raw_names.lock().unwrap().get(path) {
            return b.clone();
        }
        self.enc_new(path)
    }

    /// 마지막 이름은 항상 새로 인코딩 (새 파일·폴더, 이름 바꾸기 대상)
    pub fn enc_new(&self, path: &str) -> Vec<u8> {
        match path.rfind('/') {
            Some(i) if i + 1 < path.len() => {
                let dir = if i == 0 { "/" } else { &path[..i] };
                let mut b = if dir == "/" { b"/".to_vec() } else { self.enc(dir) };
                if !b.ends_with(b"/") {
                    b.push(b'/');
                }
                b.extend(self.enc_fresh(&path[i + 1..]));
                b
            }
            _ => self.enc_fresh(path),
        }
    }

    /// 이름 바이트 → 글자. EUC-KR 한글이 UTF-8로 우연히 유효한 경우는 거의 없으므로
    /// UTF-8로 읽히면 UTF-8, 아니면 EUC-KR(CP949)로 본다 — 섞여 있어도 둘 다 제대로 보인다.
    pub fn dec(&self, b: &[u8]) -> String {
        match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => encoding_rs::EUC_KR.decode(b).0.into_owned(),
        }
    }

    fn remember(&self, dir: &str, dir_raw: &[u8], name: &str, raw: &[u8]) {
        let mut full = dir_raw.to_vec();
        if !full.ends_with(b"/") {
            full.push(b'/');
        }
        full.extend_from_slice(raw);
        let mut m = self.codec.raw_names.lock().unwrap();
        if m.len() > 50_000 {
            m.clear();
        }
        m.insert(join(dir, name), full);
    }

    // ---- 기본 요청 ----

    pub async fn realpath(&self, path: &str) -> Result<String, String> {
        let mut p = Vec::new();
        put_str(&mut p, &self.enc(path));
        let r = self.call(FXP_REALPATH, &p).await?;
        if r.typ != FXP_NAME {
            return Err(err_of(&r));
        }
        let mut rd = Rd::new(&r.body);
        let n = rd.u32()?;
        if n == 0 {
            return Err("empty realpath".into());
        }
        Ok(self.dec(rd.bytes()?))
    }

    pub async fn stat(&self, path: &str, follow: bool) -> Result<Attrs, String> {
        let mut p = Vec::new();
        put_str(&mut p, &self.enc(path));
        let r = self.call(if follow { FXP_STAT } else { FXP_LSTAT }, &p).await?;
        if r.typ != FXP_ATTRS {
            return Err(err_of(&r));
        }
        Rd::new(&r.body).attrs()
    }

    async fn open(&self, path: &str, pflags: u32) -> Result<Vec<u8>, String> {
        let mut p = Vec::new();
        put_str(&mut p, &self.enc(path));
        put_u32(&mut p, pflags);
        put_u32(&mut p, 0); // attrs: 없음
        let r = self.call(FXP_OPEN, &p).await?;
        handle_of(&r)
    }

    async fn close(&self, h: &[u8]) -> Result<(), String> {
        let mut p = Vec::new();
        put_str(&mut p, h);
        self.call_status(FXP_CLOSE, &p).await
    }

    /// 디렉터리 목록 (원시 이름 바이트, 속성). "."과 ".."은 뺀다.
    async fn readdir_raw(&self, path: &str) -> Result<Vec<(Vec<u8>, Attrs)>, String> {
        let dir_raw = self.enc(path);
        let mut p = Vec::new();
        put_str(&mut p, &dir_raw);
        let r = self.call(FXP_OPENDIR, &p).await?;
        let h = handle_of(&r)?;
        let mut out = Vec::new();
        let res: Result<(), String> = async {
            loop {
                let mut p = Vec::new();
                put_str(&mut p, &h);
                let r = self.call(FXP_READDIR, &p).await?;
                if r.typ == FXP_STATUS {
                    let code = Rd::new(&r.body).u32()?;
                    if code == FX_EOF {
                        return Ok(());
                    }
                    return Err(status_err(&r.body));
                }
                if r.typ != FXP_NAME {
                    return Err("unexpected reply".into());
                }
                let mut rd = Rd::new(&r.body);
                let n = rd.u32()?;
                for _ in 0..n {
                    let name = rd.bytes()?.to_vec();
                    rd.bytes()?; // longname
                    let a = rd.attrs()?;
                    if name != b"." && name != b".." {
                        out.push((name, a));
                    }
                }
            }
        }
        .await;
        let _ = self.close(&h).await;
        res?;
        for (raw, _) in &out {
            self.remember(path, &dir_raw, &self.dec(raw), raw);
        }
        Ok(out)
    }

    /// 디렉터리 목록
    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        let raw = self.readdir_raw(path).await?;
        let mut out = Vec::with_capacity(raw.len());
        for (name, a) in raw {
            let name = self.dec(&name);
            let mut is_dir = a.is_dir();
            let is_link = a.is_link();
            if is_link {
                // 링크가 가리키는 대상이 폴더인지
                let full = join(path, &name);
                if let Ok(t) = self.stat(&full, true).await {
                    is_dir = t.is_dir();
                }
            }
            out.push(Entry {
                name,
                is_dir,
                is_link,
                size: a.size.unwrap_or(0),
                mtime: a.mtime.unwrap_or(0),
                perm: a.perm.unwrap_or(0) & 0o7777,
            });
        }
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), String> {
        let mut p = Vec::new();
        put_str(&mut p, &self.enc_new(path));
        put_u32(&mut p, 0);
        self.call_status(FXP_MKDIR, &p).await
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let mut p = Vec::new();
        put_str(&mut p, &self.enc(from));
        put_str(&mut p, &self.enc_new(to)); // 같은 글자로 바꿔도 서버 인코딩으로 다시 쓴다
        self.call_status(FXP_RENAME, &p).await
    }

    /// 파일 삭제, 폴더면 안의 내용까지 모두 삭제
    pub async fn remove(&self, path: &str) -> Result<(), String> {
        let a = self.stat(path, false).await?;
        if a.is_dir() {
            for (name, _) in self.readdir_raw(path).await? {
                let child = join(path, &self.dec(&name));
                Box::pin(self.remove(&child)).await?;
            }
            let mut p = Vec::new();
            put_str(&mut p, &self.enc(path));
            self.call_status(FXP_RMDIR, &p).await
        } else {
            let mut p = Vec::new();
            put_str(&mut p, &self.enc(path));
            self.call_status(FXP_REMOVE, &p).await
        }
    }

    // ---- 전송 ----

    /// 원격 파일 하나를 로컬 파일로 받는다. `prog(받은 바이트 증가분)`
    pub async fn download_file(
        &self,
        remote: &str,
        local: &Path,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        let h = self.open(remote, FXF_READ).await?;
        let res = self.download_handle(&h, local, cancel, prog).await;
        let _ = self.close(&h).await;
        if res.is_err() {
            let _ = tokio::fs::remove_file(local).await; // 받다 만 파일은 지운다
        }
        res
    }

    async fn download_handle(
        &self,
        h: &[u8],
        local: &Path,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        let mut file = tokio::fs::File::create(local).await.map_err(|e| e.to_string())?;
        // (offset, 요청 길이, 응답 수신기)
        let mut q: VecDeque<(u64, u32, oneshot::Receiver<Resp>)> = VecDeque::new();
        let mut next: u64 = 0;
        let mut eof = false;
        let read_req = |off: u64, len: u32| -> Result<oneshot::Receiver<Resp>, String> {
            let mut p = Vec::new();
            put_str(&mut p, h);
            put_u64(&mut p, off);
            put_u32(&mut p, len);
            self.send(FXP_READ, &p)
        };
        loop {
            while !eof && q.len() < WINDOW {
                q.push_back((next, CHUNK, read_req(next, CHUNK)?));
                next += CHUNK as u64;
            }
            let Some((off, len, rx)) = q.pop_front() else {
                break;
            };
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let r = rx.await.map_err(|_| "SFTP connection closed".to_string())?;
            match r.typ {
                FXP_DATA => {
                    let mut rd = Rd::new(&r.body);
                    let data = rd.bytes()?;
                    if !data.is_empty() {
                        file.seek(std::io::SeekFrom::Start(off)).await.map_err(|e| e.to_string())?;
                        file.write_all(data).await.map_err(|e| e.to_string())?;
                        prog(data.len() as u64);
                    }
                    let got = data.len() as u32;
                    if got < len {
                        // 짧게 읽힘 — 나머지 범위를 다시 요청 (EOF면 그 요청이 EOF로 답한다)
                        let off2 = off + got as u64;
                        q.push_front((off2, len - got, read_req(off2, len - got)?));
                    }
                }
                FXP_STATUS => {
                    let code = Rd::new(&r.body).u32()?;
                    if code == FX_EOF {
                        eof = true; // 이후 오프셋 요청도 EOF로 끝난다
                    } else {
                        return Err(status_err(&r.body));
                    }
                }
                _ => return Err("unexpected reply".into()),
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 로컬 파일 하나를 원격 경로로 올린다 (있으면 덮어씀).
    pub async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        let mut file = tokio::fs::File::open(local).await.map_err(|e| e.to_string())?;
        let h = self.open(remote, FXF_WRITE | FXF_CREAT | FXF_TRUNC).await?;
        let res: Result<(), String> = async {
            let mut q: VecDeque<(u64, oneshot::Receiver<Resp>)> = VecDeque::new();
            let mut off: u64 = 0;
            let mut done = false;
            let mut buf = vec![0u8; CHUNK as usize];
            loop {
                while !done && q.len() < WINDOW {
                    let n = read_full(&mut file, &mut buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        done = true;
                        break;
                    }
                    let mut p = Vec::with_capacity(n + 32);
                    put_str(&mut p, &h);
                    put_u64(&mut p, off);
                    put_str(&mut p, &buf[..n]);
                    q.push_back((n as u64, self.send(FXP_WRITE, &p)?));
                    off += n as u64;
                }
                let Some((n, rx)) = q.pop_front() else {
                    break;
                };
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let r = rx.await.map_err(|_| "SFTP connection closed".to_string())?;
                expect_ok(&r)?;
                prog(n);
            }
            Ok(())
        }
        .await;
        let closed = self.close(&h).await;
        res.and(closed)
    }
}

async fn read_full(f: &mut tokio::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..]).await?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

async fn read_packet<R: AsyncRead + Unpin>(rd: &mut R) -> std::io::Result<Vec<u8>> {
    let len = rd.read_u32().await? as usize;
    if len > 4 * 1024 * 1024 {
        return Err(std::io::Error::other("SFTP packet too large"));
    }
    let mut b = vec![0u8; len];
    rd.read_exact(&mut b).await?;
    Ok(b)
}

fn err_of(r: &Resp) -> String {
    if r.typ == FXP_STATUS {
        status_err(&r.body)
    } else {
        "unexpected reply".into()
    }
}

fn expect_ok(r: &Resp) -> Result<(), String> {
    if r.typ == FXP_STATUS && Rd::new(&r.body).u32()? == FX_OK {
        Ok(())
    } else {
        Err(err_of(r))
    }
}

fn handle_of(r: &Resp) -> Result<Vec<u8>, String> {
    if r.typ != FXP_HANDLE {
        return Err(err_of(r));
    }
    Ok(Rd::new(&r.body).bytes()?.to_vec())
}

/// 원격 경로 잇기 ("/" 구분)
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 원격 경로의 마지막 이름
pub fn base(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// 원격 경로의 상위 디렉터리 ("/a/b/c.txt" → "/a/b", 루트면 "")
pub fn base_dir(path: &str) -> &str {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => "",
        Some(i) => &p[..i],
        None => "",
    }
}

/// "이름 (2).확장자" 형태로 번호를 붙인다 (원격 충돌 회피용)
pub fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({}){}", &name[..i], n, &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

/// 로컬에 같은 이름이 있으면 "이름 (2).확장자"처럼 겹치지 않는 경로
pub fn unique_local(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let safe = if safe.trim().is_empty() || safe == "." || safe == ".." { "_".to_string() } else { safe };
    let mut p = dir.join(&safe);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match safe.rfind('.') {
        Some(i) if i > 0 => (safe[..i].to_string(), safe[i..].to_string()),
        _ => (safe.clone(), String::new()),
    };
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    p
}
