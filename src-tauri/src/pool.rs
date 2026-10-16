// 사이트 연결 + 연결 풀 (SFTP 채널 또는 FTP 제어연결).
//
// browse: 탐색/나열 전용 1개 — 대형 전송에 막히지 않게 분리
// idle:   전송 워커용 연결 풀 — 워커 수(동시 전송 수)만큼 지연 생성
// SFTP는 SSH 연결 1개 위의 채널들, FTP는 각기 독립 제어연결이지만 pool 관점에선 같다.
// 모든 연결은 같은 NameCodec(파일명 인코딩 상태)을 공유한다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::backend::FileConn;
use crate::connect::SshHandle;
use crate::ftp::FtpConn;
use crate::sftp::{NameCodec, Sftp};
use crate::tr_s;

enum Kind {
    Sftp { handle: SshHandle },
    Ftp { host: String, port: u16, user: String, pass: String },
}

pub struct SitePool {
    kind: Kind,
    pub codec: Arc<NameCodec>,
    /// 접속 당시의 표시 이름(호스트) — 상태 이벤트용
    pub host: String,
    browse: tokio::sync::Mutex<Option<Arc<dyn FileConn>>>,
    idle: Mutex<Vec<Arc<dyn FileConn>>>,
}

impl SitePool {
    pub fn new_sftp(handle: SshHandle, host: String) -> Arc<SitePool> {
        Arc::new(SitePool {
            kind: Kind::Sftp { handle },
            codec: Arc::new(NameCodec::default()),
            host,
            browse: tokio::sync::Mutex::new(None),
            idle: Mutex::new(Vec::new()),
        })
    }

    pub fn new_ftp(host: String, port: u16, user: String, pass: String) -> Arc<SitePool> {
        Arc::new(SitePool {
            kind: Kind::Ftp { host: host.clone(), port, user, pass },
            codec: Arc::new(NameCodec::default()),
            host,
            browse: tokio::sync::Mutex::new(None),
            idle: Mutex::new(Vec::new()),
        })
    }

    /// 새 연결(SFTP 채널 또는 FTP 제어연결)을 연다.
    async fn open_conn(&self) -> Result<Arc<dyn FileConn>, String> {
        match &self.kind {
            Kind::Sftp { handle } => {
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
            Kind::Ftp { host, port, user, pass } => {
                let c = FtpConn::connect_login(host, *port, user, pass, self.codec.clone()).await?;
                Ok(c as Arc<dyn FileConn>)
            }
        }
    }

    /// 탐색/나열 전용 연결 (끊겼으면 다시 연다).
    pub async fn browse(&self) -> Result<Arc<dyn FileConn>, String> {
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

    /// 전송 워커용 연결을 빌린다 — 유휴 연결 재사용, 없으면 새로 연다.
    pub async fn take_xfer(&self) -> Result<Arc<dyn FileConn>, String> {
        loop {
            let s = self.idle.lock().unwrap().pop();
            match s {
                Some(s) if !s.is_closed() => return Ok(s),
                Some(_) => {} // 끊긴 연결 폐기
                None => break,
            }
        }
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

    /// SFTP면 SSH 핸들을 돌려준다 (서버 로케일 조사용). FTP면 None.
    pub fn ssh_handle(&self) -> Option<&SshHandle> {
        match &self.kind {
            Kind::Sftp { handle } => Some(handle),
            Kind::Ftp { .. } => None,
        }
    }

    pub fn is_alive(&self) -> bool {
        match &self.kind {
            Kind::Sftp { handle } => !handle.is_closed(),
            // FTP는 영속 핸들이 없다 — 탐색 연결이 살아있거나 아직 안 열렸으면 유효로 본다
            Kind::Ftp { .. } => self
                .browse
                .try_lock()
                .map(|b| b.as_ref().map(|c| !c.is_closed()).unwrap_or(true))
                .unwrap_or(true),
        }
    }
}
