//! 多账号。当前账号就是 Claude Code 正在用的：~/.claude/.credentials.json 加 ~/.claude.json 里的 oauthAccount。
//! 其他账号各占 %APPDATA%\ClaudeTile\accounts 下的一个目录，目录就是那个账号的 CLAUDE_CONFIG_DIR
//! （添加账号时在这个目录里跑 claude auth login，凭证和 .claude.json 都写在里面）。
//!
//! 切换：先把当前账号的凭证和 oauthAccount 存回它的目录（没有就按邮箱建一个），再把目标账号的拷进 ~/.claude。
//! 当前账号的 token 由 Claude Code 刷新，它的目录里那份可能是旧的，所以列表里不显示当前账号的目录。
//!
//! 打卡：5 小时窗口没在跑时，用这个账号发一条最小的消息（haiku、不带工具和配置，约 3k token），窗口在人不在时就开始计时。

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

use crate::json::{self, Json};
use crate::quota::{Fetch, Kind};

const CREDS: &str = ".credentials.json";
const OAUTH: &str = "oauthAccount";

pub struct Account {
    pub dir: PathBuf,
    pub email: String,
    pub quota: Fetch,
    pub next_poll: i64,
    /// 下次可以打卡的时间（打卡后等额度接口反映出来）
    pub next_ping: i64,
    pub fetching: bool,
    pub pinging: bool,
}

/// Claude Code 的全局配置：用默认目录时在 ~/.claude.json，设了 CLAUDE_CONFIG_DIR 时在那个目录里
pub fn global_json(claude: &Path, home: &Path) -> PathBuf {
    if claude == home.join(".claude") { home.join(".claude.json") } else { claude.join(".claude.json") }
}

pub fn email(global: &Path) -> Option<String> {
    let j = Json::parse(&std::fs::read_to_string(global).ok()?)?;
    j.get(OAUTH).get("emailAddress").str().map(str::to_string)
}

/// accounts 下登录过的账号目录和邮箱，按邮箱排
pub fn load(root: &Path) -> Vec<(PathBuf, String)> {
    let mut v: Vec<_> = std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|d| d.join(CREDS).is_file()).filter_map(|d| Some((email(&d.join(".claude.json"))?, d))).map(|(e, d)| (d, e)).collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// 把 path 这个 JSON 文件的 oauthAccount 换成 raw，其他内容原样保留；文件不存在就新建
fn set_oauth(path: &Path, raw: &str) -> std::io::Result<()> {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    let out = match json::span(&s, OAUTH) {
        Some(r) => format!("{}{raw}{}", &s[..r.start], &s[r.end..]),
        None => match s.find('{') {
            Some(i) if s[i + 1..].trim_start().starts_with('"') => format!("{}\"{OAUTH}\":{raw},{}", &s[..=i], &s[i + 1..]),
            _ => format!("{{\"{OAUTH}\":{raw}}}"),
        },
    };
    std::fs::write(path, out)
}

/// 切到 to 目录里的账号
pub fn switch(claude: &Path, global: &Path, root: &Path, to: &Path) -> std::io::Result<()> {
    let err = || std::io::Error::other("no oauthAccount");
    let t = std::fs::read_to_string(to.join(".claude.json"))?;
    let new = &t[json::span(&t, OAUTH).ok_or_else(err)?];
    let g = std::fs::read_to_string(global)?;
    if let (Some(r), Some(e)) = (json::span(&g, OAUTH), email(global)) {
        let dir = load(root).into_iter().find(|(_, m)| *m == e).map_or_else(|| root.join(e.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_")), |(d, _)| d);
        std::fs::create_dir_all(&dir)?;
        std::fs::copy(claude.join(CREDS), dir.join(CREDS))?;
        set_oauth(&dir.join(".claude.json"), &g[r])?;
    }
    std::fs::copy(to.join(CREDS), claude.join(CREDS))?;
    set_oauth(global, new)
}

pub fn claude_exe(home: &Path) -> PathBuf {
    let p = home.join(".local").join("bin").join("claude.exe");
    if p.is_file() { p } else { "claude".into() }
}

/// 子进程的环境：dir 为 None 用默认目录（当前账号），代理照 ~/.claude/settings.json 给（账号目录里没有 settings.json）
fn env(c: &mut Command, dir: Option<&Path>, proxy: Option<&str>) {
    match dir {
        Some(d) => c.env("CLAUDE_CONFIG_DIR", d),
        None => c.env_remove("CLAUDE_CONFIG_DIR"),
    };
    if let Some(p) = proxy {
        c.env("HTTPS_PROXY", p);
    }
}

/// 5 小时窗口没在跑（或 token 过期、读不到窗口），该打卡了
pub fn idle(q: &Fetch, now: i64) -> bool {
    match q {
        Fetch::Ok(ws) => ws.iter().find(|w| w.kind == Kind::Session).is_none_or(|w| w.resets.is_none_or(|r| r <= now)),
        Fetch::Expired => true,
        _ => false,
    }
}

/// 自动打卡下次什么时候打：5 小时窗口重置时（没在跑就是现在），不早于 after；读不到额度时不知道
pub fn next_ping(q: &Fetch, after: i64, now: i64) -> Option<i64> {
    match q {
        Fetch::Ok(ws) => Some(ws.iter().find(|w| w.kind == Kind::Session).and_then(|w| w.resets).unwrap_or(now).max(after).max(now)),
        Fetch::Expired => Some(after.max(now)),
        _ => None,
    }
}

/// 打卡，返回成没成功。阻塞，最多 5 分钟，在后台线程里跑
pub fn ping(exe: &Path, dir: Option<&Path>, proxy: Option<&str>) -> bool {
    let work = std::env::temp_dir().join("claude-ping");
    let _ = std::fs::create_dir_all(&work);
    let mut c = Command::new(exe);
    // --safe-mode 不加载 CLAUDE.md / skills / 插件 / hooks / MCP，加上不带工具和极短的系统提示，约 3k token
    c.args(["-p", "--model", "haiku", "--effort", "low", "--safe-mode", "--no-session-persistence", "--tools", "", "--system-prompt", "Reply ok."]);
    c.current_dir(&work).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW.0);
    env(&mut c, dir, proxy);
    let Ok(mut ch) = c.spawn() else { return false };
    if let Some(mut i) = ch.stdin.take() {
        let _ = i.write_all(b"hi");
    }
    for _ in 0..300 {
        if let Ok(Some(s)) = ch.try_wait() {
            return s.success();
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let _ = ch.kill();
    false
}

/// 开一个命令行窗口在 dir 里登录新账号，窗口关掉后返回
pub fn login(exe: &Path, dir: &Path, proxy: Option<&str>) {
    let _ = std::fs::create_dir_all(dir);
    let mut c = Command::new("cmd.exe");
    // cmd /c 后面整串原样给它解析，exe 路径要自己加引号
    c.raw_arg(format!("/c \"\"{}\" auth login & pause\"", exe.display())).creation_flags(CREATE_NEW_CONSOLE.0);
    env(&mut c, Some(dir), proxy);
    if let Ok(mut ch) = c.spawn() {
        let _ = ch.wait();
    }
}

/// 额度条上的两个数：5 小时、每周
pub fn pcts(q: &Fetch) -> Option<(f64, f64)> {
    let Fetch::Ok(ws) = q else { return None };
    let p = |k: Kind| ws.iter().find(|w| w.kind == k).map_or(0.0, |w| w.pct);
    Some((p(Kind::Session), p(Kind::Week)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::Window;

    #[test]
    fn next() {
        let q = |r| Fetch::Ok(vec![Window { kind: Kind::Session, pct: 5.0, resets: r }]);
        // 窗口在跑：重置时；没在跑：现在，但不早于上次打卡后的等待
        assert_eq!(next_ping(&q(Some(500)), 0, 100), Some(500));
        assert_eq!(next_ping(&q(Some(50)), 0, 100), Some(100));
        assert_eq!(next_ping(&q(None), 300, 100), Some(300));
        assert_eq!(next_ping(&Fetch::Err(String::new()), 0, 100), None);
    }

    #[test]
    fn swap() {
        let t = std::env::temp_dir().join(format!("ctile-acct-{}", std::process::id()));
        let (claude, root) = (t.join(".claude"), t.join("accounts"));
        let (global, b) = (global_json(&claude, &t), root.join("new-1"));
        for d in [&claude, &b] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(claude.join(CREDS), "A").unwrap();
        std::fs::write(&global, r#"{"x":1,"oauthAccount":{"emailAddress":"a@x"},"y":[2]}"#).unwrap();
        std::fs::write(b.join(CREDS), "B").unwrap();
        std::fs::write(b.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"b@x"}}"#).unwrap();
        switch(&claude, &global, &root, &b).unwrap();
        assert_eq!(std::fs::read_to_string(claude.join(CREDS)).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(&global).unwrap(), r#"{"x":1,"oauthAccount":{"emailAddress":"b@x"},"y":[2]}"#);
        assert_eq!(load(&root), vec![(root.join("a@x"), "a@x".into()), (b.clone(), "b@x".into())]);
        // 切回去：存回已有的 b 目录，不再新建
        switch(&claude, &global, &root, &root.join("a@x")).unwrap();
        assert_eq!(std::fs::read_to_string(claude.join(CREDS)).unwrap(), "A");
        assert_eq!(email(&global).as_deref(), Some("a@x"));
        assert_eq!(load(&root).len(), 2);
        // 插到已有字段前面
        let p = t.join("g.json");
        std::fs::write(&p, r#"{ "k": 1 }"#).unwrap();
        set_oauth(&p, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"{"oauthAccount":{}, "k": 1 }"#);
        let _ = std::fs::remove_dir_all(&t);
    }
}
