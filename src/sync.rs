//! ~/.claude 的同步状态：~/.claude/sync.sh 写的 sync.log、计划任务 ClaudeSync 的下次运行时间、本地仓库和远端差几个提交。
//! 没有 sync.log 的机器（不用这套同步脚本）不显示。
//! sync.log 里每次同步以「ISO 时间 来源」开头，以 ok 或含「失败」的行结束；没有结束行的是还在跑或中途被杀。

use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::System::Variant::VARIANT;
use windows::core::BSTR;

use crate::time;

const TASK: &str = "ClaudeSync";

#[derive(Clone, Debug, PartialEq)]
pub struct Sync {
    /// 最近一次跑完的同步：开始时间（Unix 秒）、成没成功、失败原因
    pub at: i64,
    pub ok: bool,
    pub err: String,
    /// 计划任务下次运行的时间，None = 没有这个计划任务
    pub next: Option<i64>,
    /// 本地领先、落后远端几个提交，None = git 读不了
    pub diverge: Option<(u32, u32)>,
}

/// 日志里最近一次跑完的同步：(开始时间, 成功, 失败原因)
pub fn last(log: &str) -> Option<(i64, bool, String)> {
    let mut best = None;
    let mut cur: Option<(i64, String)> = None;
    for l in log.lines() {
        if let Some(t) = l.split(' ').next().and_then(time::parse_iso) {
            // 被锁跳过的不算一次同步
            cur = (!l.ends_with("skipped: locked")).then(|| (t, String::new()));
        } else if let Some((t, err)) = &mut cur {
            if l == "ok" {
                best = Some((*t, true, String::new()));
                cur = None;
            } else if l.contains("失败") {
                // git 的报错比脚本自己的那句具体，有就用它
                best = Some((*t, false, if err.is_empty() { l.to_string() } else { err.clone() }));
                cur = None;
            } else if err.is_empty() && (l.starts_with("fatal: ") || l.starts_with("error: ")) {
                *err = l[7..].to_string();
            }
        }
    }
    best
}

/// 在后台线程里调：读日志末尾、问计划任务、跑 git
pub fn load(claude: &Path) -> Option<Sync> {
    let mut f = std::fs::File::open(claude.join("sync.log")).ok()?;
    let n = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(n.saturating_sub(16 * 1024))).ok()?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    let (at, ok, err) = last(&String::from_utf8_lossy(&b))?;
    Some(Sync { at, ok, err, next: next_run(), diverge: diverge(claude) })
}

fn next_run() -> Option<i64> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let s: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).ok()?;
        let v = VARIANT::default();
        s.Connect(&v, &v, &v, &v).ok()?;
        let t = s.GetFolder(&BSTR::from("\\")).ok()?.GetTask(&BSTR::from(TASK)).ok()?;
        // NextRunTime 对带重复间隔的触发器会给出已过去的时间，按触发器从现在起往后算第一次
        let from = GetLocalTime();
        let to = SYSTEMTIME { wYear: from.wYear + 1, ..from };
        let (mut n, mut p) = (1u32, std::ptr::null_mut());
        // 任务停用、触发器都过期时 n = 0
        t.GetRunTimes(&from, &to, &mut n, &mut p).ok()?;
        let r = (n > 0).then(|| time::from_local(&*p)).flatten();
        CoTaskMemFree(Some(p as _));
        r
    }
}

fn diverge(repo: &Path) -> Option<(u32, u32)> {
    let o = Command::new("git").arg("-C").arg(repo).args(["rev-list", "--left-right", "--count", "HEAD...origin/main"]).creation_flags(CREATE_NO_WINDOW.0).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout);
    let mut n = s.split_whitespace().map(|x| x.parse().ok());
    Some((n.next()??, n.next()??))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_finished() {
        let t = time::parse_iso("2026-10-19T07:45:11-07:00").unwrap();
        let log = "Mon Oct 19 07:32:18 PDT 2026 SessionStart\nok\n\
            2026-10-19T07:40:00-07:00 SessionStart\nfatal: unable to access 'x': SSL failed\npull 失败，已 abort rebase\n\
            2026-10-19T07:45:11-07:00 schedule\nwarning: LF will be replaced\nok\n\
            2026-10-19T07:46:00-07:00 SessionEnd skipped: locked\n\
            2026-10-19T07:50:00-07:00 SessionEnd\n";
        // 07:50 那次还没结束，取 07:45 的
        assert_eq!(last(log), Some((t, true, String::new())));
        let cut = &log[..log.find("2026-10-19T07:45").unwrap()];
        assert_eq!(last(cut), Some((t - 311, false, "unable to access 'x': SSL failed".into())));
        assert_eq!(last("2026-10-19T07:40:00-07:00 x\npush 失败\n"), Some((t - 311, false, "push 失败".into())));
        assert_eq!(last("Mon Oct 19 07:32:18 PDT 2026 x\nok\n"), None);
    }
}
