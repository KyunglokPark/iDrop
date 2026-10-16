// SFTP/FTP 공통 파일 연산 트레잇.
// pool이 Arc<dyn FileConn>을 돌려주면 queue/명령은 프로토콜을 몰라도 동작한다.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;

use crate::sftp::{Attrs, Entry, Sftp};

/// 하나의 전송/탐색 연결(SFTP 채널 또는 FTP 제어연결)이 제공하는 연산.
#[async_trait]
pub trait FileConn: Send + Sync {
    fn is_closed(&self) -> bool;
    async fn realpath(&self, path: &str) -> Result<String, String>;
    async fn stat(&self, path: &str, follow: bool) -> Result<Attrs, String>;
    async fn list(&self, path: &str) -> Result<Vec<Entry>, String>;
    async fn mkdir(&self, path: &str) -> Result<(), String>;
    async fn rename(&self, from: &str, to: &str) -> Result<(), String>;
    async fn remove(&self, path: &str) -> Result<(), String>;
    /// offset>0이면 대상의 기존 부분 바이트 뒤에서 이어받는다 (이어받기).
    async fn download_file(
        &self,
        remote: &str,
        local: &Path,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String>;
    async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String>;
}

#[async_trait]
impl FileConn for Sftp {
    fn is_closed(&self) -> bool {
        Sftp::is_closed(self)
    }
    async fn realpath(&self, path: &str) -> Result<String, String> {
        Sftp::realpath(self, path).await
    }
    async fn stat(&self, path: &str, follow: bool) -> Result<Attrs, String> {
        Sftp::stat(self, path, follow).await
    }
    async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        Sftp::list(self, path).await
    }
    async fn mkdir(&self, path: &str) -> Result<(), String> {
        Sftp::mkdir(self, path).await
    }
    async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        Sftp::rename(self, from, to).await
    }
    async fn remove(&self, path: &str) -> Result<(), String> {
        Sftp::remove(self, path).await
    }
    async fn download_file(
        &self,
        remote: &str,
        local: &Path,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        Sftp::download_file(self, remote, local, offset, cancel, prog).await
    }
    async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        offset: u64,
        cancel: &AtomicBool,
        prog: &mut (dyn FnMut(u64) + Send),
    ) -> Result<(), String> {
        Sftp::upload_file(self, local, remote, offset, cancel, prog).await
    }
}
