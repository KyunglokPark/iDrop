// Windows 전용 기능 — 크레이트 버전에 묶이지 않게 필요한 Win32 API만 직접 선언한다.
//  - DPAPI: 저장된 비밀번호 암호화 (현재 Windows 사용자 계정에 묶임)
//  - 파일 열기 / 폴더 선택 대화상자 (SFTP 업로드·다운로드, 개인키 파일 고르기)
// 다른 OS에서는 지원하지 않음 에러를 돌려준다.
// (idot_check: 리눅스에서 타입 검사만 할 때 Windows 코드를 함께 검사하는 cfg)

#[cfg(any(windows, idot_check))]
mod imp {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            descr: *const u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            descr: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
    // 같은 PC의 다른 프로그램이 같은 사용자 권한으로 풀지 못하게 하는 추가 엔트로피
    const ENTROPY: &[u8] = b"iDOT.password.v1";

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = DataBlob { cb_data: plain.len() as u32, pb_data: plain.as_ptr() as *mut u8 };
        let ent = DataBlob { cb_data: ENTROPY.len() as u32, pb_data: ENTROPY.as_ptr() as *mut u8 };
        let mut out = DataBlob { cb_data: 0, pb_data: null_mut() };
        let ok = unsafe {
            CryptProtectData(&input, null(), &ent, null_mut(), null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err("CryptProtectData failed".into());
        }
        let v = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut c_void) };
        Ok(v)
    }

    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
        let input = DataBlob { cb_data: cipher.len() as u32, pb_data: cipher.as_ptr() as *mut u8 };
        let ent = DataBlob { cb_data: ENTROPY.len() as u32, pb_data: ENTROPY.as_ptr() as *mut u8 };
        let mut out = DataBlob { cb_data: 0, pb_data: null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, null_mut(), &ent, null_mut(), null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err("CryptUnprotectData failed".into());
        }
        let v = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe { LocalFree(out.pb_data as *mut c_void) };
        Ok(v)
    }

    // ---- 파일 대화상자 -----------------------------------------------------------

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: isize,
        h_instance: isize,
        lpstr_filter: *const u16,
        lpstr_custom_filter: *mut u16,
        n_max_cust_filter: u32,
        n_filter_index: u32,
        lpstr_file: *mut u16,
        n_max_file: u32,
        lpstr_file_title: *mut u16,
        n_max_file_title: u32,
        lpstr_initial_dir: *const u16,
        lpstr_title: *const u16,
        flags: u32,
        n_file_offset: u16,
        n_file_extension: u16,
        lpstr_def_ext: *const u16,
        l_cust_data: isize,
        lpfn_hook: *const c_void,
        lp_template_name: *const u16,
        pv_reserved: *mut c_void,
        dw_reserved: u32,
        flags_ex: u32,
    }

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: isize,
        pidl_root: *const c_void,
        psz_display_name: *mut u16,
        lpsz_title: *const u16,
        ul_flags: u32,
        lpfn: *const c_void,
        l_param: isize,
        i_image: i32,
    }

    #[link(name = "comdlg32")]
    extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SHBrowseForFolderW(bi: *const BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoTaskMemFree(pv: *mut c_void);
        fn OleInitialize(reserved: *mut c_void) -> i32;
        fn OleUninitialize();
    }
    #[link(name = "user32")]
    extern "system" {
        fn EnableWindow(hwnd: isize, enable: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn AllowSetForegroundWindow(pid: u32) -> i32;
    }

    /// 대화상자는 OLE(STA)가 초기화된 스레드에서 띄운다. 오래된 탐색기 확장 프로그램
    /// (파일 선택 창 안에 함께 로드된다)이 OLE를 기대하는 경우가 있어 OleInitialize를 쓴다.
    pub fn on_sta_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let hr = unsafe { OleInitialize(null_mut()) };
                let r = f();
                if hr >= 0 {
                    unsafe { OleUninitialize() };
                }
                r
            })
            .expect("dialog thread")
            .join()
            .expect("dialog thread")
    }

    /// 대화상자 도우미 프로세스가 앞으로 나올 수 있게 허용한다.
    pub fn allow_foreground() {
        unsafe { AllowSetForegroundWindow(u32::MAX /* ASFW_ANY */) };
    }

    /// 도우미 프로세스가 비정상 종료돼도 주인 창이 비활성으로 남지 않게 되살린다.
    pub fn restore_owner(owner: isize) {
        if owner != 0 {
            unsafe {
                EnableWindow(owner, 1);
                SetForegroundWindow(owner);
            }
        }
    }

    const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
    const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
    const OFN_EXPLORER: u32 = 0x0008_0000;
    const OFN_HIDEREADONLY: u32 = 0x0000_0004;
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
    const BIF_NEWDIALOGSTYLE: u32 = 0x0040;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 파일 열기 대화상자. `filter`는 ("설명", "*.ext;*.ext") 목록. 취소하면 빈 목록.
    pub fn pick_files(owner: isize, title: &str, filter: &[(String, String)], multi: bool) -> Vec<String> {
        let mut filt: Vec<u16> = Vec::new();
        for (name, pat) in filter {
            filt.extend(name.encode_utf16());
            filt.push(0);
            filt.extend(pat.encode_utf16());
            filt.push(0);
        }
        filt.push(0);
        let title_w = wide(title);
        let mut buf = vec![0u16; 65536];
        let mut ofn = OpenFileNameW {
            l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            hwnd_owner: owner,
            h_instance: 0,
            lpstr_filter: if filter.is_empty() { null() } else { filt.as_ptr() },
            lpstr_custom_filter: null_mut(),
            n_max_cust_filter: 0,
            n_filter_index: 1,
            lpstr_file: buf.as_mut_ptr(),
            n_max_file: buf.len() as u32,
            lpstr_file_title: null_mut(),
            n_max_file_title: 0,
            lpstr_initial_dir: null(),
            lpstr_title: title_w.as_ptr(),
            flags: OFN_EXPLORER
                | OFN_PATHMUSTEXIST
                | OFN_FILEMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_HIDEREADONLY
                | if multi { OFN_ALLOWMULTISELECT } else { 0 },
            n_file_offset: 0,
            n_file_extension: 0,
            lpstr_def_ext: null(),
            l_cust_data: 0,
            lpfn_hook: null(),
            lp_template_name: null(),
            pv_reserved: null_mut(),
            dw_reserved: 0,
            flags_ex: 0,
        };
        if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
            return Vec::new();
        }
        // 여러 개: "폴더\0파일1\0파일2\0\0", 하나: "전체경로\0\0"
        let parts: Vec<String> = buf
            .split(|&c| c == 0)
            .take_while(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect();
        match parts.len() {
            0 => Vec::new(),
            1 => parts,
            _ => {
                let dir = parts[0].trim_end_matches('\\').to_string();
                parts[1..].iter().map(|f| format!("{dir}\\{f}")).collect()
            }
        }
    }

    /// 폴더 선택 대화상자. 취소하면 None.
    pub fn pick_folder(owner: isize, title: &str) -> Option<String> {
        let title_w = wide(title);
        let mut name = vec![0u16; 260];
        let bi = BrowseInfoW {
            hwnd_owner: owner,
            pidl_root: null(),
            psz_display_name: name.as_mut_ptr(),
            lpsz_title: title_w.as_ptr(),
            ul_flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
            lpfn: null(),
            l_param: 0,
            i_image: 0,
        };
        let pidl = unsafe { SHBrowseForFolderW(&bi) };
        if pidl.is_null() {
            return None;
        }
        let mut path = vec![0u16; 32768];
        let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
        unsafe { CoTaskMemFree(pidl) };
        if ok == 0 {
            return None;
        }
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        Some(String::from_utf16_lossy(&path[..end]))
    }
}

#[cfg(not(any(windows, idot_check)))]
mod imp {
    pub fn protect(_p: &[u8]) -> Result<Vec<u8>, String> {
        Err("unsupported".into())
    }
    pub fn unprotect(_c: &[u8]) -> Result<Vec<u8>, String> {
        Err("unsupported".into())
    }
    pub fn pick_files(_o: isize, _t: &str, _f: &[(String, String)], _m: bool) -> Vec<String> {
        Vec::new()
    }
    pub fn pick_folder(_o: isize, _t: &str) -> Option<String> {
        None
    }
    pub fn on_sta_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        f()
    }
    pub fn allow_foreground() {}
    pub fn restore_owner(_o: isize) {}
}

pub use imp::{allow_foreground, on_sta_thread, pick_files, pick_folder, protect, restore_owner, unprotect};

use base64::Engine as _;

const PREFIX: &str = "dpapi:";

/// 평문 → "dpapi:<base64>" (이미 암호문이면 그대로, 빈 값은 빈 값)
pub fn protect_str(s: &str) -> Result<String, String> {
    if s.is_empty() || s.starts_with(PREFIX) {
        return Ok(s.to_string());
    }
    let c = protect(s.as_bytes())?;
    Ok(format!("{PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(c)))
}

/// "dpapi:<base64>" → 평문 (암호문이 아니면 그대로). 풀 수 없으면 Err.
pub fn unprotect_str(s: &str) -> Result<String, String> {
    let Some(b64) = s.strip_prefix(PREFIX) else {
        return Ok(s.to_string());
    };
    let c = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| e.to_string())?;
    let p = unprotect(&c)?;
    String::from_utf8(p).map_err(|e| e.to_string())
}
