//! 正在运行的会话：读 Claude Code 自己维护的 sessions/<进程号>.json（在 ~/.claude 下；Orca 里起的在它各账号档的目录下）。
//! 每个 claude 进程一份，status 是 busy / idle / waiting（waitingFor 写着在等什么），CLI 和桌面版都有。
//! 进程崩溃时文件会留下来，所以按进程号和启动时间（procStart）确认进程还在。
//! 桌面版会话的标题在 %APPDATA%\Claude\claude-code-sessions 里，按 cliSessionId 对上。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::json::Json;
use crate::sys;
use crate::usage::project_of;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting(String),
    Busy,
    Idle,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub pid: u32,
    pub id: String,
    pub project: String,
    /// 会话名（Claude Code 按目录起的，如 claudeclidiy-05）
    pub name: String,
    pub desktop: bool,
    pub state: State,
    /// 进入当前状态的时间（Unix 秒）
    pub since: i64,
}

impl Session {
    pub fn rank(&self) -> u8 {
        match self.state {
            State::Waiting(_) => 0,
            State::Busy => 1,
            State::Idle => 2,
        }
    }
}

pub fn parse(s: &str) -> Option<Session> {
    let j = Json::parse(s)?;
    let state = match j.get("status").str()? {
        "busy" => State::Busy,
        "waiting" => State::Waiting(j.get("waitingFor").str().unwrap_or("").to_string()),
        _ => State::Idle,
    };
    let at = j.get("statusUpdatedAt").num().or(j.get("updatedAt").num()).unwrap_or(0.0);
    Some(Session {
        pid: j.get("pid").u64() as u32,
        id: j.get("sessionId").str()?.to_string(),
        project: project_of(j.get("cwd").str().unwrap_or("")),
        name: j.get("name").str().unwrap_or("").to_string(),
        desktop: j.get("entrypoint").str().is_some_and(|e| e != "cli"),
        state,
        since: (at / 1000.0) as i64,
    })
}

/// 进程还活着的会话，等你处理的排最前，其次是在跑的，同类里最近变化的在前
pub fn load(dirs: &[PathBuf]) -> Vec<Session> {
    let mut v: Vec<Session> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let s = std::fs::read_to_string(e.path()).ok()?;
            let start = Json::parse(&s)?.get("procStart").str().and_then(|p| p.parse().ok());
            parse(&s).filter(|x| sys::alive(x.pid, start))
        })
        .collect();
    v.sort_by(|a, b| a.rank().cmp(&b.rank()).then(b.since.cmp(&a.since)));
    v
}

/// 桌面版会话标题：cliSessionId → title
pub fn desktop_titles(dir: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for a in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
                if !f.file_name().to_string_lossy().starts_with("local_") {
                    continue;
                }
                let Some(j) = std::fs::read_to_string(f.path()).ok().and_then(|s| Json::parse(&s)) else { continue };
                if let (Some(id), Some(t)) = (j.get("cliSessionId").str(), j.get("title").str()) {
                    m.insert(id.to_string(), t.to_string());
                }
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status() {
        let s = parse(r#"{"pid":32652,"sessionId":"abc","cwd":"C:\\WorkFile\\X","entrypoint":"cli","name":"x-05","status":"waiting","waitingFor":"dialog open","updatedAt":1792336596569,"statusUpdatedAt":1792336590000}"#).unwrap();
        assert_eq!((s.pid, s.project.as_str(), s.desktop, s.since), (32652, "X", false, 1792336590));
        assert_eq!(s.state, State::Waiting("dialog open".into()));
        let d = parse(r#"{"pid":1,"sessionId":"d","cwd":"C:\\Y","entrypoint":"claude-desktop","status":"busy","updatedAt":1000}"#).unwrap();
        assert_eq!((d.desktop, d.state, d.since), (true, State::Busy, 1));
        assert!(parse(r#"{"pid":1}"#).is_none());
    }
}
