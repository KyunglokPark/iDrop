// 사이트 연결 + SFTP 채널 풀.
//
// SSH 연결은 사이트당 1개. 그 위에 SFTP 채널을 용도별로 나눈다:
//  - browse: 탐색/나열 전용 1개 — 대형 전송에 막히지 않게 분리
//  - xfer:   전송 워커용 채널 풀 — 워커 수(동시 전송 수)만큼 지연 생성
// 모든 채널은 같은 NameCodec(파일명 인코딩 상태)을 공유한다. 탐색 채널이 목록에서
// 캐시한 EUC-KR 원시 바이트를 전송 채널이 그대로 쓰기 위한 것 (계획서 §2.4).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::connect::SshHandle;
use crate::sftp::{NameCodec, Sftp};
use crate::tr_s;

pub struct SitePool {
    pub handle: SshHandle,
    pub codec: Arc<NameCodec>,
    /// 접속 당시의 표시 이름(호스트) — 상태 이벤트용
    pub host: String,
    browse: tokio::sync::Mutex<Option<Arc<Sftp>>>,
    idle: Mutex<Vec<Arc<Sftp>>>,
    /// 지금까지 만든 전송 채널 수 (끊긴 채널은 반납 시 감산)
    live: AtomicUsize,
}

impl SitePool {
    pub fn new(handle: SshHandle, host: String) -> Arc<SitePool> {
        Arc::new(SitePool {
            handle,
            codec: Arc::new(NameCodec::default()),
            host,
            browse: tokio::sync::Mutex::new(None),
            idle: Mutex::new(Vec::new()),
            live: AtomicUsize::new(0),
        })
    }

    /// 새 SFTP 채널을 연다 (sftp 서브시스템 + INIT).
    async fn open_channel(&self) -> Result<Arc<Sftp>, String> {
        let ch = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| tr_s!(format!("SFTP 채널 열기 실패: {e}"), format!("failed to open SFTP channel: {e}")))?;
        ch.request_subsystem(true, "sftp")
            .await
            .map_err(|e| tr_s!(format!("SFTP 시작 실패: {e}"), format!("failed to start SFTP: {e}")))?;
        tokio::time::timeout(Duration::from_secs(15), Sftp::start(ch.into_stream(), self.codec.clone()))
            .await
            .map_err(|_| {
                tr_s!(
                    "SFTP 응답 없음 (서버에서 SFTP가 꺼져 있을 수 있습니다)",
                    "no SFTP response (SFTP may be disabled on the server)"
                )
            })?
    }

    /// 탐색/나열 전용 채널 (끊겼으면 다시 연다).
    pub async fn browse(&self) -> Result<Arc<Sftp>, String> {
        let mut b = self.browse.lock().await;
        if let Some(s) = b.as_ref() {
            if !s.is_closed() {
                return Ok(s.clone());
            }
        }
        let s = self.open_channel().await?;
        *b = Some(s.clone());
        Ok(s)
    }

    /// 전송 워커용 채널을 빌린다 — 유휴 채널 재사용, 없으면 새로 연다.
    /// (워커 수 자체가 동시 전송 설정으로 제한되므로 여기서 상한을 두지 않는다)
    pub async fn take_xfer(&self) -> Result<Arc<Sftp>, String> {
        loop {
            let s = self.idle.lock().unwrap().pop();
            match s {
                Some(s) if !s.is_closed() => return Ok(s),
                Some(_) => {
                    self.live.fetch_sub(1, Ordering::Relaxed); // 끊긴 채널 폐기
                }
                None => break,
            }
        }
        let s = self.open_channel().await?;
        self.live.fetch_add(1, Ordering::Relaxed);
        Ok(s)
    }

    /// 전송 채널 반납.
    pub fn give_back(&self, s: Arc<Sftp>) {
        if s.is_closed() {
            self.live.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        let mut idle = self.idle.lock().unwrap();
        // 유휴 채널을 과하게 쌓지 않는다 (동시 전송 상한 8개면 충분)
        if idle.len() < 8 {
            idle.push(s);
        } else {
            self.live.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.handle.is_closed()
    }
}
