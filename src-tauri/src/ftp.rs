// 최소 FTP 클라이언트 (RFC 959 + MLSD/EPSV). tokio 네이티브, 단일 런타임 유지.
//
// 제어연결(포트 21)로 명령을 주고받고, 전송·목록은 PASV/EPSV 데이터연결로 한다.
// 한 연결은 한 번에 한 작업만 하므로 내부 상태를 Mutex로 감싼다 (전송 동시성은
// pool이 연결을 여러 개 빌려주는 방식으로 얻는다 — SFTP 채널 풀과 같은 구조).
// 파일 이름은 바이트로 다룬다: 서버가 EUC-KR이면 그 인코딩으로 보내고, 목록은
// UTF-8로 읽히면 UTF-8, 아니면 EUC-KR로 디코드한다 (NameCodec.euc 플래그 사용).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::io::AsyncBufReadExt;
use tokio::net::TcpStream;

use crate::backend::FileConn;
use crate::sftp::{Attrs, Entry, NameCodec};
use crate::tr_s;

const CHUNK: usize = 64 * 1024;

struct Inner {
    ctrl: BufReader<TcpStream>,
    closed: bool,
}

pub struct FtpConn {
    inner: tokio::sync::Mutex<Inner>,
    codec: Arc<NameCodec>,
}

fn io_err(e: impl std::fmt::Display) -> String {
    tr_s!(format!("FTP 통신 오류: {e}"), format!("FTP I/O error: {e}"))
}

impl Inner {
    /// 한 응답을 읽는다 (여러 줄이면 마지막 "코드 " 줄까지). (코드, 전체 텍스트)
    async fn read_reply(&mut self) -> Result<(u16, String), String> {
        let mut first = String::new();
        self.ctrl.read_line(&mut first).await.map_err(io_err)?;
        if first.len() < 4 {
            self.closed = true;
            return Err(io_err("빈 응답"));
        }
        let code: u16 = first[..3].parse().map_err(|_| io_err("잘못된 응답 코드"))?;
        let mut text = first.clone();
        // 여러 줄 응답: "123-" 로 시작하면 "123 " 줄이 나올 때까지 계속 읽는다
        if first.as_bytes().get(3) == Some(&b'-') {
            let end = format!("{code} ");
            loop {
                let mut line = String::new();
                if self.ctrl.read_line(&mut line).await.map_err(io_err)? == 0 {
                    break;
                }
                text.push_str(&line);
                if line.starts_with(&end) {
                    break;
                }
            }
        }
        Ok((code, text))
    }

    /// 명령을 보내고 응답을 받는다. 2xx/1xx가 아니면 에러.
    async fn cmd(&mut self, line: &[u8]) -> Result<(u16, String), String> {
        self.ctrl.get_mut().write_all(line).await.map_err(io_err)?;
        self.ctrl.get_mut().write_all(b"\r\n").await.map_err(io_err)?;
        self.ctrl.get_mut().flush().await.map_err(io_err)?;
        let (code, text) = self.read_reply().await?;
        Ok((code, text))
    }

    async fn expect(&mut self, line: &[u8], ok: &[u16]) -> Result<String, String> {
        let (code, text) = self.cmd(line).await?;
        if ok.contains(&code) {
            Ok(text)
        } else {
            Err(ftp_err(&text, code))
        }
    }

    /// 데이터연결을 연다 (EPSV 우선, 실패 시 PASV).
    async fn open_data(&mut self) -> Result<TcpStream, String> {
        // EPSV: 229 Entering Extended Passive Mode (|||port|)
        if let Ok((229, text)) = self.cmd(b"EPSV").await {
            if let Some(port) = parse_epsv(&text) {
                let peer = self.ctrl.get_ref().peer_addr().map_err(io_err)?;
                let addr = SocketAddr::new(peer.ip(), port);
                if let Ok(s) = TcpStream::connect(addr).await {
                    return Ok(s);
                }
            }
        }
        // PASV: 227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)
        let (code, text) = self.cmd(b"PASV").await?;
        if code != 227 {
            return Err(ftp_err(&text, code));
        }
        let (ip, port) = parse_pasv(&text).ok_or_else(|| io_err("PASV 응답 해석 실패"))?;
        // NAT 뒤 서버가 사설 IP를 돌려주면 제어연결의 공인 IP를 쓴다
        let peer = self.ctrl.get_ref().peer_addr().map_err(io_err)?;
        let target = if is_private(ip) && !is_private_sock(peer.ip()) {
            SocketAddr::new(peer.ip(), port)
        } else {
            SocketAddr::new(IpAddr::V4(ip), port)
        };
        TcpStream::connect(target).await.map_err(io_err)
    }
}

fn ftp_err(text: &str, code: u16) -> String {
    let msg = text.lines().next().map(|l| l.trim()).unwrap_or("").to_string();
    tr_s!(format!("FTP {code}: {msg}"), format!("FTP {code}: {msg}"))
}

fn parse_epsv(text: &str) -> Option<u16> {
    let s = text.find('(')?;
    let e = text[s..].find(')')? + s;
    let inner = &text[s + 1..e]; // |||port|
    inner.trim_matches('|').split('|').next_back()?.trim().parse().ok()
}

fn parse_pasv(text: &str) -> Option<(Ipv4Addr, u16)> {
    let s = text.find('(')?;
    let e = text[s..].find(')')? + s;
    let nums: Vec<u8> = text[s + 1..e].split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if nums.len() != 6 {
        return None;
    }
    let ip = Ipv4Addr::new(nums[0], nums[1], nums[2], nums[3]);
    let port = (nums[4] as u16) << 8 | nums[5] as u16;
    Some((ip, port))
}

fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_unspecified()
}
fn is_private_sock(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private(v4),
        IpAddr::V6(_) => false,
    }
}

impl FtpConn {
    /// 접속 + 로그인. 서버가 UTF-8을 받으면 켠다.
    pub async fn connect_login(
        host: &str,
        port: u16,
        user: &str,
        pass: &str,
        codec: Arc<NameCodec>,
    ) -> Result<Arc<FtpConn>, String> {
        let stream = tokio::time::timeout(
            Duration::from_secs(15),
            TcpStream::connect((host, port)),
        )
        .await
        .map_err(|_| tr_s!(format!("{host}:{port} 연결 시간 초과"), format!("{host}:{port} connection timed out")))?
        .map_err(|e| tr_s!(format!("연결 실패: {e}"), format!("connection failed: {e}")))?;
        let _ = stream.set_nodelay(true);
        let mut inner = Inner {
            ctrl: BufReader::new(stream),
            closed: false,
        };
        let (code, text) = inner.read_reply().await?; // 220 greeting
        if code != 220 {
            return Err(ftp_err(&text, code));
        }
        // 로그인
        let (c1, t1) = inner.cmd(format!("USER {user}").as_bytes()).await?;
        if c1 == 331 {
            inner.expect(format!("PASS {pass}").as_bytes(), &[230, 202]).await?;
        } else if c1 != 230 {
            return Err(ftp_err(&t1, c1));
        }
        // 바이너리 전송 + UTF-8 옵션(지원하면)
        let _ = inner.cmd(b"TYPE I").await;
        if !codec.euc.load(Ordering::Relaxed) {
            let _ = inner.cmd(b"OPTS UTF8 ON").await; // 실패해도 무시
        }
        Ok(Arc::new(FtpConn {
            inner: tokio::sync::Mutex::new(inner),
            codec,
        }))
    }

    fn enc(&self, name: &str) -> Vec<u8> {
        if self.codec.euc.load(Ordering::Relaxed) {
            encoding_rs::EUC_KR.encode(name).0.into_owned()
        } else {
            name.as_bytes().to_vec()
        }
    }

    fn dec(&self, b: &[u8]) -> String {
        match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => encoding_rs::EUC_KR.decode(b).0.into_owned(),
        }
    }

    /// 명령 + 경로(바이트) 한 줄 구성
    fn line(&self, verb: &str, path: &str) -> Vec<u8> {
        let mut v = format!("{verb} ").into_bytes();
        v.extend_from_slice(&self.enc(path));
        v
    }
}

#[async_trait]
impl FileConn for FtpConn {
    fn is_closed(&self) -> bool {
        self.inner.try_lock().map(|g| g.closed).unwrap_or(false)
    }

    async fn realpath(&self, path: &str) -> Result<String, String> {
        let mut g = self.inner.lock().await;
        if !path.is_empty() && path != "." {
            let line = {
                let mut v = b"CWD ".to_vec();
                v.extend_from_slice(&self.enc(path));
                v
            };
            g.expect(&line, &[250, 200]).await?;
        }
        let text = g.expect(b"PWD", &[257]).await?;
        // 257 "/path" ...
        let p = text.find('"').and_then(|s| {
            text[s + 1..].find('"').map(|e| text[s + 1..s + 1 + e].to_string())
        });
        Ok(p.unwrap_or_else(|| "/".to_string()))
    }

    async fn stat(&self, path: &str, _follow: bool) -> Result<Attrs, String> {
        // 디렉터리면 CWD가 성공, 파일이면 SIZE로 크기 — 존재 확인 겸용
        let mut g = self.inner.lock().await;
        let sz_line = self.line("SIZE", path);
        if let Ok((213, text)) = g.cmd(&sz_line).await {
            let size = text[3..].trim().parse::<u64>().ok();
            return Ok(Attrs { size, perm: Some(0o100644), mtime: None });
        }
        // SIZE 실패 → 디렉터리인지 CWD로 확인 (원위치 복원은 안 함: browse 전용 연결)
        let cwd = self.line("CWD", path);
        match g.cmd(&cwd).await {
            Ok((250, _)) | Ok((200, _)) => Ok(Attrs { size: None, perm: Some(0o040755), mtime: None }),
            _ => Err(tr_s!("경로를 찾을 수 없습니다", "no such path")),
        }
    }

    async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        let mut g = self.inner.lock().await;
        if !path.is_empty() && path != "." {
            let cwd = self.line("CWD", path);
            g.expect(&cwd, &[250, 200]).await?;
        }
        // MLSD(기계 판독) 우선, 안 되면 LIST
        let data = g.open_data().await?;
        let (code, _text) = g.cmd(b"MLSD").await?;
        let use_mlsd = code == 150 || code == 125;
        if !use_mlsd {
            // MLSD 미지원 — LIST로 재시도
            let data2 = g.open_data().await?;
            let (c2, t2) = g.cmd(b"LIST -a").await?;
            if c2 != 150 && c2 != 125 {
                return Err(ftp_err(&t2, c2));
            }
            let raw = read_all(data2).await?;
            g.expect(b"", &[226, 250]).await.ok();
            // 빈 명령으로 완료 응답만 읽는다
            return Ok(self.parse_list(&raw, false));
        }
        let raw = read_all(data).await?;
        let (_c, _t) = g.read_reply().await?; // 226 완료
        Ok(self.parse_list(&raw, true))
    }

    async fn mkdir(&self, path: &str) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let line = self.line("MKD", path);
        g.expect(&line, &[257, 250]).await.map(|_| ())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let f = self.line("RNFR", from);
        g.expect(&f, &[350]).await?;
        let tt = self.line("RNTO", to);
        g.expect(&tt, &[250, 200]).await.map(|_| ())
    }

    async fn remove(&self, path: &str) -> Result<(), String> {
        // 재귀 삭제: 먼저 파일 DELE 시도, 실패하면 디렉터리로 보고 하위부터 지운다
        let is_dir = {
            let mut g = self.inner.lock().await;
            let dele = self.line("DELE", path);
            match g.cmd(&dele).await {
                Ok((250, _)) => return Ok(()),
                _ => true,
            }
        };
        if is_dir {
            for ent in self.list(path).await? {
                let child = crate::sftp::join(path, &ent.name);
                Box::pin(self.remove(&child)).await?;
            }
            let mut g = self.inner.lock().await;
            let rmd = self.line("RMD", path);
            g.expect(&rmd, &[250]).await.map(|_| ())
        } else {
            Ok(())
        }
    }

    async fn download_file(
        &self,
        remote: &str,
        local: &Path,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let mut data = g.open_data().await?;
        // 이어받기: RETR 직전에 REST로 재시작 지점을 알린다 (350 기대).
        if offset > 0 {
            let (c, t) = g.cmd(format!("REST {offset}").as_bytes()).await?;
            if c != 350 {
                return Err(ftp_err(&t, c));
            }
        }
        let retr = self.line("RETR", remote);
        let (code, text) = g.cmd(&retr).await?;
        if code != 150 && code != 125 {
            return Err(ftp_err(&text, code));
        }
        // offset>0: 기존 부분 파일을 자르지 않고 그 지점으로 seek. 0: 새로 만든다.
        let mut file = if offset > 0 {
            let mut f = tokio::fs::OpenOptions::new().write(true).open(local).await.map_err(io_err)?;
            f.seek(std::io::SeekFrom::Start(offset)).await.map_err(io_err)?;
            f
        } else {
            tokio::fs::File::create(local).await.map_err(io_err)?
        };
        let mut buf = vec![0u8; CHUNK];
        let mut aborted = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                aborted = true;
                break;
            }
            let n = data.read(&mut buf).await.map_err(io_err)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).await.map_err(io_err)?;
            prog(n as u64);
        }
        drop(data);
        file.flush().await.map_err(io_err)?;
        if aborted {
            // 부분 파일은 남겨 둔다 (재시도 시 이어받기용). 서버 전송만 중단.
            let _ = g.cmd(b"ABOR").await;
            return Err(tr_s!("전송이 취소되었습니다", "transfer cancelled"));
        }
        g.expect(b"", &[226, 250]).await.ok();
        let _ = g.read_reply().await; // 완료 응답 흡수 (서버별 타이밍 차이)
        Ok(())
    }

    async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        let mut file = tokio::fs::File::open(local).await.map_err(io_err)?;
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset)).await.map_err(io_err)?;
        }
        let mut g = self.inner.lock().await;
        let mut data = g.open_data().await?;
        // 이어받기: STOR 직전에 REST로 재시작 지점을 알린다 (350 기대).
        if offset > 0 {
            let (c, t) = g.cmd(format!("REST {offset}").as_bytes()).await?;
            if c != 350 {
                return Err(ftp_err(&t, c));
            }
        }
        let stor = self.line("STOR", remote);
        let (code, text) = g.cmd(&stor).await?;
        if code != 150 && code != 125 {
            return Err(ftp_err(&text, code));
        }
        let mut buf = vec![0u8; CHUNK];
        let mut aborted = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                aborted = true;
                break;
            }
            let n = file.read(&mut buf).await.map_err(io_err)?;
            if n == 0 {
                break;
            }
            data.write_all(&buf[..n]).await.map_err(io_err)?;
            prog(n as u64);
        }
        let _ = data.flush().await;
        drop(data); // 데이터연결 닫음 = 전송 종료 신호
        if aborted {
            let _ = g.cmd(b"ABOR").await;
            return Err(tr_s!("전송이 취소되었습니다", "transfer cancelled"));
        }
        let (c, t) = g.read_reply().await?; // 226 완료
        if c != 226 && c != 250 {
            return Err(ftp_err(&t, c));
        }
        Ok(())
    }
}

impl FtpConn {
    /// MLSD 또는 LIST 응답을 Entry 목록으로 파싱한다.
    fn parse_list(&self, raw: &[u8], mlsd: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        for line in raw.split(|&b| b == b'\n') {
            let line: &[u8] = if line.last() == Some(&b'\r') { &line[..line.len() - 1] } else { line };
            if line.is_empty() {
                continue;
            }
            let entry = if mlsd { self.parse_mlsd(line) } else { self.parse_unix(line) };
            if let Some(e) = entry {
                if e.name != "." && e.name != ".." {
                    out.push(e);
                }
            }
        }
        out.sort_by(|a, b| (b.is_dir as u8).cmp(&(a.is_dir as u8)).then(a.name.cmp(&b.name)));
        out
    }

    // "type=dir;size=123;modify=20240101120000; 이름"
    fn parse_mlsd(&self, line: &[u8]) -> Option<Entry> {
        let sp = line.iter().position(|&b| b == b' ')?;
        let facts = std::str::from_utf8(&line[..sp]).ok()?.to_ascii_lowercase();
        let name = self.dec(&line[sp + 1..]);
        let mut is_dir = false;
        let mut is_link = false;
        let mut size = 0u64;
        let mut mtime = 0u32;
        for fact in facts.split(';') {
            let (k, v) = fact.split_once('=').unwrap_or((fact, ""));
            match k {
                "type" => {
                    is_dir = v == "dir" || v == "cdir" || v == "pdir";
                    is_link = v.starts_with("os.unix=slink") || v == "link";
                }
                "size" => size = v.parse().unwrap_or(0),
                "modify" => mtime = parse_mlsd_time(v),
                _ => {}
            }
        }
        if name.is_empty() {
            return None;
        }
        Some(Entry { name, is_dir, is_link, size, mtime, perm: if is_dir { 0o040755 } else { 0o100644 } })
    }

    // Unix `ls -l`: "drwxr-xr-x 2 u g 4096 Jan 17 2025 name"
    fn parse_unix(&self, line: &[u8]) -> Option<Entry> {
        if line.len() < 10 {
            return None;
        }
        let first = line[0];
        let is_dir = first == b'd';
        let is_link = first == b'l';
        // 공백으로 8필드 분리 후 나머지가 이름 (8번째 필드 시작 위치를 바이트로 찾는다)
        let mut field = 0;
        let mut i = 0;
        let mut in_space = false;
        let mut name_start = None;
        while i < line.len() {
            let sp = line[i] == b' ' || line[i] == b'\t';
            if sp && !in_space {
                in_space = true;
            } else if !sp && in_space {
                in_space = false;
                field += 1;
                if field == 8 {
                    name_start = Some(i);
                    break;
                }
            }
            i += 1;
        }
        let ns = name_start?;
        let mut name = self.dec(&line[ns..]);
        // 심볼릭 링크 "name -> target"에서 대상 제거
        if is_link {
            if let Some(p) = name.find(" -> ") {
                name = name[..p].to_string();
            }
        }
        // 크기 = 5번째 필드 (0-index 4)
        let text = String::from_utf8_lossy(line);
        let size = text.split_whitespace().nth(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        if name.is_empty() {
            return None;
        }
        Some(Entry { name, is_dir, is_link, size, mtime: 0, perm: if is_dir { 0o040755 } else { 0o100644 } })
    }
}

fn parse_mlsd_time(v: &str) -> u32 {
    // YYYYMMDDHHMMSS → epoch 초 (대략, UTC)
    if v.len() < 14 {
        return 0;
    }
    let g = |a: usize, b: usize| v[a..b].parse::<i64>().unwrap_or(0);
    let (y, mo, d, h, mi, s) = (g(0, 4), g(4, 6), g(6, 8), g(8, 10), g(10, 12), g(12, 14));
    // 간이 그레고리력 → epoch (윤년 포함)
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0 { 366 } else { 365 };
    }
    let mdays = [31, if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(mo - 1).max(0) as usize {
        if m < 12 {
            days += mdays[m];
        }
    }
    days += d - 1;
    (days * 86400 + h * 3600 + mi * 60 + s).max(0) as u32
}

async fn read_all(mut s: TcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.map_err(io_err)?;
    Ok(buf)
}
