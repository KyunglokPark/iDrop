// 사이트 연결 + 연결 풀 (SFTP 채널 또는 FTP 제어연결) + 자동 재연결.
//
// browse: 탐색/나열 전용 1개 — 대형 전송에 막히지 않게 분리
// idle:   전송 워커용 연결 풀 — 워커 수(동시 전송 수)만큼 지연 생성
// SFTP는 SSH 연결 1개 위의 채널들, FTP는 각기 독립 제어연결이지만 pool 관점에선 같다.
// 모든 연결은 같은 NameCodec(파일명 인코딩 상태)을 공유한다.
//
// 자동 재연결: 접속 정보(Hop)를 보관해, 연결이 끊기면 ensure_alive()가 조용히 다시 붙는다.
// SFTP는 단일 SSH 핸들이 죽으면 전부 죽으므로 핸들을 제자리 교체한다. FTP는 연결마다
// 독립이라 탐색 연결만 다시 연다. 사용자가 명시적으로 끊으면(shutdown) 재연결하지 않는다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::AppHandle;

use crate::backend::FileConn;
use crate::connect::{self, Hop, SshHandle};
use crate::ftp::FtpConn;
use crate::sftp::{NameCodec, Sftp};
use crate::tr_s;

/// 살아있는 연결 기본체. SFTP는 영속 SSH 핸들, FTP는 핸들이 없다(연결마다 로그인).
enum Conn {
    Sftp(SshHandle),
    Ftp,
}

pub struct SitePool {
    conn: tokio::sync::RwLock<Conn>,
    /// 재연결용 접속 정보 (불변)
    app: AppHandle,
    hop: Hop,
    is_ftp: bool,
    pub codec: Arc<NameCodec>,
    /// 접속 당시의 표시 이름(호스트) — 상태 이벤트용
    pub host: String,
    /// 사용자가 명시적으로 끊었는지 (스케줄러/재연결 중단 신호)
    shutdown: AtomicBool,
    /// 재연결 직렬화 (동시 다발 실패 시 한 번만 재접속)
    reconnecting: tokio::sync::Mutex<()>,
    browse: tokio::sync::Mutex<Option<Arc<dyn FileConn>>>,
    idle: Mutex<Vec<Arc<dyn FileConn>>>,
}

impl SitePool {
    pub fn new_sftp(app: AppHandle, hop: Hop, handle: SshHandle) -> Arc<SitePool> {
        let host = hop.host.clone();
        Arc::new(SitePool {
            conn: tokio::sync::RwLock::new(Conn::Sftp(handle)),
            app,
            hop,
            is_ftp: false,
            codec: Arc::new(NameCodec::default()),
            host,
            shutdown: AtomicBool::new(false),
            reconnecting: tokio::sync::Mutex::new(()),
            browse: tokio::sync::Mutex::new(None),
            idle: Mutex::new(Vec::new()),
        })
    }

    pub fn new_ftp(app: AppHandle, hop: Hop) -> Arc<SitePool> {
        let host = hop.host.clone();
        Arc::new(SitePool {
            conn: tokio::sync::RwLock::new(Conn::Ftp),
            app,
            hop,
            is_ftp: true,
            codec: Arc::new(NameCodec::default()),
            host,
            shutdown: AtomicBool::new(false),
            reconnecting: tokio::sync::Mutex::new(()),
            browse: tokio::sync::Mutex::new(None),
            idle: Mutex::new(Vec::new()),
        })
    }

    /// 새 연결(SFTP 채널 또는 FTP 제어연결)을 연다.
    async fn open_conn(&self) -> Result<Arc<dyn FileConn>, String> {
        let g = self.conn.read().await;
        match &*g {
            Conn::Sftp(handle) => {
                let ch = handle
                    .channel_open_session()
                    .await
                    .map_err(|e| tr_s!(format!("SFTP 채널 열기 실패: {e}"), format!("failed to open SFTP channel: {e}")))?;
                ch.request_subsystem(true, "sftp")
                    .await
                    .map_err(|e| tr_s!(format!("SFTP 시작 실패: {e}"), format!("failed to start SFTP: {e}")))?;
                let s = tokio::time::timeout(
                    Duration::from_secs(15),
                    Sftp::start(ch.into_stream(), self.codec.clone()),
                )
                .await
                .map_err(|_| {
                    tr_s!(
                        "SFTP 응답 없음 (서버에서 SFTP가 꺼져 있을 수 있습니다)",
                        "no SFTP response (SFTP may be disabled on the server)"
                    )
                })??;
                Ok(s as Arc<dyn FileConn>)
            }
            Conn::Ftp => {
                let c = FtpConn::connect_login(
                    &self.hop.host,
                    self.hop.port,
                    &self.hop.username,
                    &self.hop.password,
                    self.codec.clone(),
                )
                .await?;
                Ok(c as Arc<dyn FileConn>)
            }
        }
    }

    /// 연결이 살아있는지 확인하고, 끊겼으면 재연결한다 (직렬화됨).
    /// 사용자가 끊었거나(shutdown) 재접속에 실패하면 Err.
    pub async fn ensure_alive(&self) -> Result<(), String> {
        if self.is_alive() {
            return Ok(());
        }
        if self.is_shutdown() {
            return Err(tr_s!("연결이 종료되었습니다", "connection closed"));
        }
        let _g = self.reconnecting.lock().await; // 한 번에 하나만 재접속
        if self.is_alive() {
            return Ok(()); // 기다리는 동안 다른 태스크가 복구함
        }
        crate::emit_site(&self.app, "connecting", &self.host, None, None);
        match self.reconnect_once().await {
            Ok(()) => {
                crate::emit_site(&self.app, "connected", &self.host, None, None);
                Ok(())
            }
            Err(e) => {
                crate::emit_site(&self.app, "disconnected", &self.host, None, Some(e.clone()));
                Err(e)
            }
        }
    }

    /// 재접속 1회. SFTP는 새 SSH 핸들로 교체, FTP는 탐색 연결을 새로 연다.
    async fn reconnect_once(&self) -> Result<(), String> {
        if self.is_ftp {
            // 죽은 연결 폐기 후 새 로그인으로 유효성 확인
            *self.browse.lock().await = None;
            self.idle.lock().unwrap().clear();
            let c = self.open_conn().await?;
            *self.browse.lock().await = Some(c);
        } else {
            let (handle, _user) = connect::connect_and_auth(&self.app, "reconnect", &self.hop).await?;
            *self.conn.write().await = Conn::Sftp(handle);
            // 옛 채널들은 죽었다 — 다음 사용 때 새로 열린다
            *self.browse.lock().await = None;
            self.idle.lock().unwrap().clear();
        }
        Ok(())
    }

    /// 탐색/나열 전용 연결 (끊겼으면 재연결 후 다시 연다).
    pub async fn browse(&self) -> Result<Arc<dyn FileConn>, String> {
        {
            let b = self.browse.lock().await;
            if let Some(s) = b.as_ref() {
                if !s.is_closed() {
                    return Ok(s.clone());
                }
            }
        }
        self.ensure_alive().await?;
        let mut b = self.browse.lock().await;
        if let Some(s) = b.as_ref() {
            if !s.is_closed() {
                return Ok(s.clone());
            }
        }
        let s = self.open_conn().await?;
        *b = Some(s.clone());
        Ok(s)
    }

    /// 전송 워커용 연결을 빌린다 — 유휴 연결 재사용, 없으면 (재연결 후) 새로 연다.
    pub async fn take_xfer(&self) -> Result<Arc<dyn FileConn>, String> {
        loop {
            let s = self.idle.lock().unwrap().pop();
            match s {
                Some(s) if !s.is_closed() => return Ok(s),
                Some(_) => {} // 끊긴 연결 폐기
                None => break,
            }
        }
        self.ensure_alive().await?;
        self.open_conn().await
    }

    /// 전송 연결 반납.
    pub fn give_back(&self, s: Arc<dyn FileConn>) {
        if s.is_closed() {
            return;
        }
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < 8 {
            idle.push(s);
        }
    }

    /// 사용자가 명시적으로 끊었음을 표시 (재연결·재시도 중단, 스케줄러 종료 신호).
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn is_alive(&self) -> bool {
        match self.conn.try_read() {
            Ok(g) => match &*g {
                Conn::Sftp(h) => !h.is_closed(),
                // FTP는 영속 핸들이 없다 — 탐색 연결이 살아있거나 아직 안 열렸으면 유효로 본다
                Conn::Ftp => self
                    .browse
                    .try_lock()
                    .map(|b| b.as_ref().map(|c| !c.is_closed()).unwrap_or(true))
                    .unwrap_or(true),
            },
            // 쓰기 잠금 중(재연결 중) → 일단 살아있다고 본다
            Err(_) => true,
        }
    }
}
