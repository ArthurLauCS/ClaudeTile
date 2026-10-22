//! 系统相关的小工具：进程存活、切到会话所在的窗口、目录监视、开机自启。

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Console::{AttachConsole, FreeConsole, GetConsoleWindow, GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
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

/// 切到会话所在的窗口：Orca 里的会话切到 Orca 里它那个标签；终端里的会话借它的控制台找到终端窗口
/// （Windows Terminal 只能切到窗口，切不到标签页）；找不到控制台（桌面版）就沿父进程往上找第一个有可见主窗口的进程
pub fn focus(pid: u32) -> bool {
    let Some(w) = orca_window(pid).or_else(|| console_window(pid)).or_else(|| ancestor_window(pid)) else { return false };
    unsafe {
        if IsIconic(w).as_bool() {
            let _ = ShowWindow(w, SW_RESTORE);
        }
        SetForegroundWindow(w).as_bool()
    }
}

fn console_window(pid: u32) -> Option<HWND> {
    unsafe {
        // AttachConsole 会把标准句柄换成那个控制台的，FreeConsole 后失效的句柄仍留在本进程；
        // 不还原的话，之后起的子进程（SSH 设置窗口、账号登录窗口）继承它们会报“句柄无效”
        let std = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|n| (n, GetStdHandle(n).unwrap_or_default()));
        let _ = FreeConsole();
        AttachConsole(pid).ok()?;
        let h = GetConsoleWindow();
        let _ = FreeConsole();
        for (n, s) in std {
            let _ = SetStdHandle(n, s);
        }
        if h.is_invalid() {
            return None;
        }
        // Windows Terminal 给的是它名下的一个隐藏伪窗口，真正的终端窗口是它的所有者
        let root = GetAncestor(h, GA_ROOTOWNER);
        [root, h].into_iter().find(|w| !w.is_invalid() && IsWindowVisible(*w).as_bool())
    }
}

/// Orca 里的会话挂在 Orca 的后台进程下，往上找不到窗口：按它环境变量里的终端句柄让 Orca 切到那个标签，返回 Orca 的主窗口
/// （orca terminal switch 只切标签，不把窗口提到前台）
fn orca_window(pid: u32) -> Option<HWND> {
    let env = env_of(pid)?;
    let t = env.get("ORCA_TERMINAL_HANDLE")?;
    let cli = env.get("PATH")?.split(';').map(|d| Path::new(d).join("orca.exe")).find(|p| p.is_file())?;
    Command::new(cli).args(["terminal", "switch", "--terminal", t]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW.0).spawn().ok()?;
    processes().into_iter().filter(|(_, _, exe)| exe.eq_ignore_ascii_case("Orca.exe")).find_map(|(p, _, _)| main_window(p))
}

/// 另一个进程的环境变量（只读它 PEB 里的环境块），变量名转成大写
fn env_of(pid: u32) -> Option<HashMap<String, String>> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let read = |at: usize, n: usize| {
            let mut b = vec![0u8; n];
            ReadProcessMemory(h, at as _, b.as_mut_ptr() as _, n, None).ok().map(|_| b)
        };
        let ptr = |at: usize| read(at, 8).map(|b| usize::from_le_bytes(b.try_into().unwrap()));
        let mut pbi = PROCESS_BASIC_INFORMATION::default();
        let env = NtQueryInformationProcess(h, ProcessBasicInformation, &mut pbi as *mut _ as _, size_of_val(&pbi) as u32, &mut 0).is_ok().then(|| {
            // x64：PEB+0x20 是 ProcessParameters，它的 +0x80 是环境块地址、+0x3F0 是环境块字节数（读的是别的进程，长度设个上限）
            let p = ptr(pbi.PebBaseAddress as usize + 0x20)?;
            read(ptr(p + 0x80)?, ptr(p + 0x3F0)?.min(1 << 20))
        });
        let _ = CloseHandle(h);
        let w: Vec<u16> = env??.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&w).split('\0').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_uppercase(), v.to_string())).collect())
    }
}

/// 所有进程：(进程号, 父进程号, exe 名)
fn processes() -> Vec<(u32, u32, String)> {
    let mut v = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return v };
        let mut pe = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let n = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                v.push((pe.th32ProcessID, pe.th32ParentProcessID, String::from_utf16_lossy(&pe.szExeFile[..n])));
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    v
}

fn ancestor_window(pid: u32) -> Option<HWND> {
    let parents: HashMap<u32, u32> = processes().into_iter().map(|(p, pp, _)| (p, pp)).collect();
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

#[cfg(test)]
mod tests {
    #[test]
    fn env_of_self() {
        let e = super::env_of(std::process::id()).unwrap();
        assert_eq!(e.get("PATH"), std::env::var("PATH").ok().as_ref());
    }
}
