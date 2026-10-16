// FTP 백엔드 e2e: 미니 FTP 서버를 띄우고 iDrop의 실제 ftp.rs 클라이언트로 왕복 검증.
// (실제 FTP 서버를 WSL에 설치할 수 없어, SSH testserver와 같은 자급 방식으로 검증한다.)
//
// 검증: 로그인 → PWD/CWD → MLSD 목록 → STOR 업로드 → RETR 다운로드 해시 일치
//       → MKD → RNFR/RNTO → DELE/RMD.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use sha2::{Digest, Sha256};

// ---- 실제 ftp.rs를 포함하기 위한 최소 shim (crate::backend/sftp/tr_s) --------

#[macro_export]
macro_rules! tr_s {
    ($ko:expr, $en:expr) => {
        String::from($ko)
    };
}

#[path = "../../../../src-tauri/src/sftp.rs"]
pub mod sftp;

pub mod backend {
    use crate::sftp::{Attrs, Entry};
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    #[async_trait]
    pub trait FileConn: Send + Sync {
        fn is_closed(&self) -> bool;
        async fn realpath(&self, path: &str) -> Result<String, String>;
        async fn stat(&self, path: &str, follow: bool) -> Result<Attrs, String>;
        async fn list(&self, path: &str) -> Result<Vec<Entry>, String>;
        async fn mkdir(&self, path: &str) -> Result<(), String>;
        async fn rename(&self, from: &str, to: &str) -> Result<(), String>;
        async fn remove(&self, path: &str) -> Result<(), String>;
        async fn download_file(&self, remote: &str, local: &Path, offset: u64, cancel: &AtomicBool, prog: &mut (dyn FnMut(u64) + Send)) -> Result<(), String>;
        async fn upload_file(&self, local: &Path, remote: &str, offset: u64, cancel: &AtomicBool, prog: &mut (dyn FnMut(u64) + Send)) -> Result<(), String>;
    }
}

#[path = "../../../../src-tauri/src/ftp.rs"]
pub mod ftp;

use backend::FileConn;
use sftp::NameCodec;

// ---- 미니 FTP 서버 ----------------------------------------------------------

mod server {
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    pub async fn run(root: PathBuf) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let root = root.clone();
                tokio::spawn(handle(sock, root));
            }
        });
        port
    }

    async fn handle(sock: TcpStream, root: PathBuf) {
        let mut ctrl = BufReader::new(sock);
        let mut cwd = PathBuf::from("/");
        let mut rnfr: Option<String> = None;
        let mut rest: u64 = 0; // REST로 지정된 재시작 offset
        let _ = ctrl.get_mut().write_all(b"220 mini-ftp\r\n").await;
        // PASV 데이터 리스너 (다음 전송용)
        let mut pending_data: Option<TcpListener> = None;
        loop {
            let mut line = String::new();
            if ctrl.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end();
            let (cmd, arg) = match line.split_once(' ') {
                Some((c, a)) => (c.to_ascii_uppercase(), a.to_string()),
                None => (line.to_ascii_uppercase(), String::new()),
            };
            let w = ctrl.get_mut();
            match cmd.as_str() {
                "USER" => { let _ = w.write_all(b"331 pw?\r\n").await; }
                "PASS" => { let _ = w.write_all(b"230 ok\r\n").await; }
                "TYPE" => { let _ = w.write_all(b"200 ok\r\n").await; }
                "OPTS" => { let _ = w.write_all(b"200 ok\r\n").await; }
                "PWD" => {
                    let _ = w.write_all(format!("257 \"{}\"\r\n", cwd.to_string_lossy()).as_bytes()).await;
                }
                "CWD" => {
                    let np = norm(&cwd, &arg);
                    if root_join(&root, &np).is_dir() {
                        cwd = np;
                        let _ = w.write_all(b"250 ok\r\n").await;
                    } else {
                        let _ = w.write_all(b"550 no dir\r\n").await;
                    }
                }
                "EPSV" => { let _ = w.write_all(b"500 no epsv\r\n").await; }
                "PASV" => {
                    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let p = l.local_addr().unwrap().port();
                    pending_data = Some(l);
                    let (hi, lo) = (p >> 8, p & 0xff);
                    let _ = w.write_all(format!("227 Entering Passive Mode (127,0,0,1,{hi},{lo})\r\n").as_bytes()).await;
                }
                "MLSD" => {
                    let l = pending_data.take().unwrap();
                    let _ = w.write_all(b"150 here\r\n").await;
                    let (mut data, _) = l.accept().await.unwrap();
                    let dir = root_join(&root, &cwd);
                    let mut out = String::new();
                    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
                        while let Ok(Some(e)) = rd.next_entry().await {
                            let meta = e.metadata().await.unwrap();
                            let ty = if meta.is_dir() { "dir" } else { "file" };
                            out.push_str(&format!("type={};size={}; {}\r\n", ty, meta.len(), e.file_name().to_string_lossy()));
                        }
                    }
                    let _ = data.write_all(out.as_bytes()).await;
                    let _ = data.shutdown().await;
                    drop(data);
                    let _ = ctrl.get_mut().write_all(b"226 done\r\n").await;
                }
                "SIZE" => {
                    let p = root_join(&root, &norm(&cwd, &arg));
                    match tokio::fs::metadata(&p).await {
                        Ok(m) if m.is_file() => { let _ = w.write_all(format!("213 {}\r\n", m.len()).as_bytes()).await; }
                        _ => { let _ = w.write_all(b"550 no\r\n").await; }
                    }
                }
                "REST" => {
                    rest = arg.trim().parse().unwrap_or(0);
                    let _ = w.write_all(b"350 rest ok\r\n").await;
                }
                "STOR" => {
                    let l = pending_data.take().unwrap();
                    let path = root_join(&root, &norm(&cwd, &arg));
                    let off = std::mem::take(&mut rest);
                    let _ = w.write_all(b"150 send\r\n").await;
                    let (mut data, _) = l.accept().await.unwrap();
                    let mut buf = Vec::new();
                    let _ = data.read_to_end(&mut buf).await;
                    // REST offset부터 이어 쓴다 (off=0이면 통째로 덮어씀)
                    let mut f = tokio::fs::OpenOptions::new().create(true).write(true).open(&path).await.unwrap();
                    if off == 0 {
                        let _ = f.set_len(0).await;
                    }
                    f.seek(std::io::SeekFrom::Start(off)).await.unwrap();
                    f.write_all(&buf).await.unwrap();
                    let _ = ctrl.get_mut().write_all(b"226 stored\r\n").await;
                }
                "RETR" => {
                    let l = pending_data.take().unwrap();
                    let path = root_join(&root, &norm(&cwd, &arg));
                    let off = std::mem::take(&mut rest) as usize;
                    let data_v = tokio::fs::read(&path).await;
                    let _ = w.write_all(b"150 sending\r\n").await;
                    let (mut data, _) = l.accept().await.unwrap();
                    if let Ok(v) = data_v {
                        let start = off.min(v.len());
                        let _ = data.write_all(&v[start..]).await;
                    }
                    let _ = data.shutdown().await;
                    drop(data);
                    let _ = ctrl.get_mut().write_all(b"226 sent\r\n").await;
                }
                "MKD" => {
                    let _ = tokio::fs::create_dir(root_join(&root, &norm(&cwd, &arg))).await;
                    let _ = w.write_all(b"257 made\r\n").await;
                }
                "DELE" => {
                    let p = root_join(&root, &norm(&cwd, &arg));
                    if tokio::fs::remove_file(&p).await.is_ok() {
                        let _ = w.write_all(b"250 del\r\n").await;
                    } else {
                        let _ = w.write_all(b"550 no file\r\n").await;
                    }
                }
                "RMD" => {
                    let _ = tokio::fs::remove_dir_all(root_join(&root, &norm(&cwd, &arg))).await;
                    let _ = w.write_all(b"250 rmd\r\n").await;
                }
                "RNFR" => { rnfr = Some(arg); let _ = w.write_all(b"350 rnfr\r\n").await; }
                "RNTO" => {
                    if let Some(from) = rnfr.take() {
                        let _ = tokio::fs::rename(root_join(&root, &norm(&cwd, &from)), root_join(&root, &norm(&cwd, &arg))).await;
                        let _ = w.write_all(b"250 renamed\r\n").await;
                    } else {
                        let _ = w.write_all(b"503 bad\r\n").await;
                    }
                }
                "QUIT" => { let _ = w.write_all(b"221 bye\r\n").await; break; }
                _ => { let _ = w.write_all(b"502 unimpl\r\n").await; }
            }
        }
    }

    fn norm(cwd: &Path, arg: &str) -> PathBuf {
        if arg.starts_with('/') {
            PathBuf::from(arg)
        } else {
            cwd.join(arg)
        }
    }
    fn root_join(root: &Path, p: &Path) -> PathBuf {
        let rel = p.strip_prefix("/").unwrap_or(p);
        root.join(rel)
    }
}

// ---- 테스트 -----------------------------------------------------------------

#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join("idrop-ftp-e2e");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let port = server::run(dir.clone()).await;

    let codec = Arc::new(NameCodec::default());
    let c = crate::ftp::FtpConn::connect_login("127.0.0.1", port, "u", "p", codec).await.expect("로그인");

    let home = c.realpath(".").await.expect("pwd");
    assert_eq!(home, "/", "홈={home}");

    // 업로드
    let src = dir.join("local.bin");
    let data: Vec<u8> = (0..300_000u32).map(|i| i.wrapping_mul(2654435761).to_le_bytes()[1]).collect();
    std::fs::write(&src, &data).unwrap();
    let cancel = AtomicBool::new(false);
    let mut acc = 0u64;
    c.upload_file(&src, "/up.bin", 0, &cancel, &mut |d| acc += d).await.expect("upload");
    assert_eq!(acc, data.len() as u64);

    // 목록
    let list = c.list("/").await.expect("list");
    assert!(list.iter().any(|e| e.name == "up.bin" && !e.is_dir), "목록에 up.bin: {:?}", list.iter().map(|e| &e.name).collect::<Vec<_>>());

    // 다운로드 해시 일치
    let dl: PathBuf = dir.join("dl.bin");
    c.download_file("/up.bin", &dl, 0, &cancel, &mut |_| {}).await.expect("download");
    let h1 = { let mut h = Sha256::new(); h.update(&data); h.finalize() };
    let h2 = { let mut h = Sha256::new(); h.update(std::fs::read(&dl).unwrap()); h.finalize() };
    assert_eq!(h1, h2, "왕복 해시 불일치");

    // ---- 이어받기 (REST) 검증 -------------------------------------------------
    // 업로드 이어받기: 원격에 절반만 올라간 상태를 만들고 offset부터 재개.
    let half = (data.len() / 2) as u64;
    {
        // 원격 /resume.bin 에 앞 절반만 기록 (중단 상태 모사)
        let partial = dir.join("partial-src");
        std::fs::write(&partial, &data[..half as usize]).unwrap();
        c.upload_file(&partial, "/resume.bin", 0, &cancel, &mut |_| {}).await.expect("partial up");
        assert_eq!(c.stat("/resume.bin", true).await.unwrap().size, Some(half), "부분 업로드 크기");
        // 이어받기: half부터 끝까지
        let mut up2 = 0u64;
        c.upload_file(&src, "/resume.bin", half, &cancel, &mut |d| up2 += d).await.expect("resume up");
        assert_eq!(up2, data.len() as u64 - half, "이어받은 바이트=나머지");
        let back = dir.join("resume-up-back.bin");
        c.download_file("/resume.bin", &back, 0, &cancel, &mut |_| {}).await.expect("verify dl");
        let hb = { let mut h = Sha256::new(); h.update(std::fs::read(&back).unwrap()); h.finalize() };
        assert_eq!(h1, hb, "업로드 이어받기 해시 불일치");
    }
    // 다운로드 이어받기: 로컬에 절반만 받은 상태에서 offset부터 재개.
    {
        let rdl = dir.join("resume-dl.bin");
        std::fs::write(&rdl, &data[..half as usize]).unwrap(); // 받다 만 부분 파일 모사
        let mut dn2 = 0u64;
        c.download_file("/up.bin", &rdl, half, &cancel, &mut |d| dn2 += d).await.expect("resume dl");
        assert_eq!(dn2, data.len() as u64 - half, "이어받은 바이트=나머지");
        let hd = { let mut h = Sha256::new(); h.update(std::fs::read(&rdl).unwrap()); h.finalize() };
        assert_eq!(h1, hd, "다운로드 이어받기 해시 불일치");
    }

    // mkdir / rename / delete
    c.mkdir("/sub").await.expect("mkdir");
    c.rename("/up.bin", "/renamed.bin").await.expect("rename");
    assert!(c.stat("/renamed.bin", true).await.is_ok(), "rename 후 stat");
    c.remove("/renamed.bin").await.expect("delete file");
    c.remove("/sub").await.expect("rmdir");

    println!("PASS ftp_roundtrip: 로그인 + MLSD 목록 + 업/다운 해시 일치 + mkdir/rename/delete + REST 이어받기(업/다운)");
}
