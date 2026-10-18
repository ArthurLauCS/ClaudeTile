//! 系统相关的小工具：进程存活、切到会话所在的窗口、目录监视、开机自启。

use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleWindow};
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_NAME: PCWSTR = w!("ClaudeTile");

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// 进程还在，且启动时间对得上（防止进程号被别的进程复用）；start 是 FILETIME（1601 起的 100ns 数）
pub fn alive(pid: u32, start: Option<u64>) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let running = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let (mut c, mut e, mut k, mut u) = Default::default();
        let same = start.is_none_or(|s| {
            GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok() && (((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64) as i64 - s as i64).abs() < 10_000_000
        });
        let _ = CloseHandle(h);
        running && same
    }
}

/// 切到会话所在的窗口：终端里的会话借它的控制台找到终端窗口（Windows Terminal 只能切到窗口，切不到标签页），
/// 找不到控制台（桌面版）就沿父进程往上找第一个有可见主窗口的进程
pub fn focus(pid: u32) -> bool {
    let Some(w) = console_window(pid).or_else(|| ancestor_window(pid)) else { return false };
    unsafe {
        if IsIconic(w).as_bool() {
            let _ = ShowWindow(w, SW_RESTORE);
        }
        SetForegroundWindow(w).as_bool()
    }
}

fn console_window(pid: u32) -> Option<HWND> {
    unsafe {
        let _ = FreeConsole();
        AttachConsole(pid).ok()?;
        let h = GetConsoleWindow();
        let _ = FreeConsole();
        if h.is_invalid() {
            return None;
        }
        // Windows Terminal 给的是它名下的一个隐藏伪窗口，真正的终端窗口是它的所有者
        let root = GetAncestor(h, GA_ROOTOWNER);
        [root, h].into_iter().find(|w| !w.is_invalid() && IsWindowVisible(*w).as_bool())
    }
}

fn ancestor_window(pid: u32) -> Option<HWND> {
    let parents = unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut pe = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut m = std::collections::HashMap::new();
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                m.insert(pe.th32ProcessID, pe.th32ParentProcessID);
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        m
    };
    let mut p = pid;
    for _ in 0..8 {
        if let Some(w) = main_window(p) {
            return Some(w);
        }
        p = *parents.get(&p)?;
    }
    None
}

fn main_window(pid: u32) -> Option<HWND> {
    struct Find(u32, Option<HWND>);
    unsafe extern "system" fn cb(w: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let f = &mut *(lp.0 as *mut Find);
            let mut p = 0;
            GetWindowThreadProcessId(w, Some(&mut p));
            if p == f.0 && IsWindowVisible(w).as_bool() && GetWindow(w, GW_OWNER).is_err() && GetWindowTextLengthW(w) > 0 {
                f.1 = Some(w);
                return FALSE;
            }
            TRUE
        }
    }
    let mut f = Find(pid, None);
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut f as *mut _ as isize));
    }
    f.1
}

/// 后台线程监视目录（recursive 含子目录），有变化就给 notify 窗口发 msg；不解析具体变化，主线程去抖后整体重读
pub fn watch(dir: PathBuf, recursive: bool, notify: HWND, msg: u32) {
    let notify = notify.0 as usize;
    std::thread::spawn(move || unsafe {
        let p = wide(&dir.to_string_lossy());
        let Ok(h) = CreateFileW(PCWSTR(p.as_ptr()), FILE_LIST_DIRECTORY.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, None) else {
            return;
        };
        let mut buf = vec![0u32; 4096];
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
        loop {
            let mut n = 0;
            if ReadDirectoryChangesW(h, buf.as_mut_ptr() as _, (buf.len() * 4) as u32, recursive, filter, Some(&mut n), None, None).is_err() {
                return;
            }
            let _ = PostMessageW(Some(HWND(notify as _)), msg, WPARAM(0), LPARAM(0));
        }
    });
}

pub fn exe_path() -> String {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

pub fn autostart() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let cmd = wide(&format!("\"{}\"", exe_path()));
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, REG_SZ.0, Some(cmd.as_ptr() as _), (cmd.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME);
        }
    }
}

pub fn reg_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let (mut v, mut n) = (0u32, 4u32);
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut n)) };
    (r == ERROR_SUCCESS).then_some(v)
}
