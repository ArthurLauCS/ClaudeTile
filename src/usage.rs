//! Token 用量：扫描 ~/.claude/projects 下所有对话记录（*.jsonl，含子 agent），按本地日期、项目、模型汇总。
//!
//! - 同一条回复按内容块拆成好几行写入，每行带同一份 usage，按 message.id 去重。
//! - 文件只读新增部分：记住每个文件读到的位置，只推进到最后一个换行（半行等下次）。
//! - Claude Code 默认删除 30 天前的对话记录，所以每天的汇总另存一份（history.txt），
//!   和重新扫描的结果按项取较大值合并：删记录只会让数变小，取大值不丢历史，也不会重复计数。
//! - 顺带记下每个会话的标题（custom-title / ai-title）和最近一次调用的工具，给会话列表用。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::json::Json;
use crate::time;

/// 输入、输出、缓存写入、缓存读取
pub type Tok = [u64; 4];

/// 天号 → (项目, 模型) → 用量
pub type Days = BTreeMap<i32, HashMap<(String, String), Tok>>;

#[derive(Clone, Default)]
pub struct SessionInfo {
    pub title: Option<String>,
    /// 标题是用户自己起的（/rename），之后 AI 起的标题不覆盖它
    custom: bool,
    /// 最近一次调用的工具，如 "Bash: cargo build"
    pub tool: Option<String>,
}

#[derive(Default)]
pub struct Scanner {
    root: PathBuf,
    offsets: HashMap<PathBuf, u64>,
    seen: HashSet<u64>,
    /// 工作目录 → 项目名，找 git 根目录要碰磁盘，每个目录只找一次
    projects: HashMap<String, String>,
    pub days: Days,
    pub sessions: HashMap<String, SessionInfo>,
}

pub fn total(t: &Tok, cache_read: bool) -> u64 {
    t[0] + t[1] + t[2] + if cache_read { t[3] } else { 0 }
}

fn add(a: &mut Tok, b: &Tok) {
    for i in 0..4 {
        a[i] += b[i];
    }
}

fn jsonl_files(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(md) = e.metadata() else { continue };
        let p = e.path();
        if md.is_dir() {
            jsonl_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push((p, md.len()));
        }
    }
}

/// 路径最后一段（Windows 和 / 分隔都认）
pub fn base_name(p: &str) -> &str {
    p.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(p)
}

/// 项目名：所在 git 仓库的目录名（在子目录里开的会话也归到仓库），不在仓库里就用目录名；
/// 桌面版没选文件夹时在临时目录（scratch-workspaces）里跑，统一归成一项
pub fn project_of(cwd: &str) -> String {
    if cwd.contains("scratch-workspaces") {
        return DESKTOP_SCRATCH.to_string();
    }
    let p = Path::new(cwd);
    let root = p.ancestors().take(6).find(|a| a.join(".git").exists()).unwrap_or(p);
    base_name(&root.to_string_lossy()).to_string()
}

pub const DESKTOP_SCRATCH: &str = "Claude Desktop";

/// 工具调用的简短说明：取最能看出在干什么的那个参数
fn tool_brief(name: &str, input: &Json) -> String {
    let arg = ["description", "command", "file_path", "pattern", "url", "query", "prompt"].iter().find_map(|k| input.get(k).str());
    match arg {
        Some(a) => {
            let a = a.lines().next().unwrap_or("");
            let a = if a.contains(['\\', '/']) && !a.contains(' ') { base_name(a) } else { a };
            format!("{name}: {}", a.chars().take(80).collect::<String>())
        }
        None => name.to_string(),
    }
}

impl Scanner {
    pub fn new(root: PathBuf) -> Scanner {
        Scanner { root, ..Default::default() }
    }

    /// 读所有文件的新增部分，返回是否读到了新的用量或标题
    pub fn scan(&mut self) -> bool {
        let mut files = Vec::new();
        jsonl_files(&self.root, &mut files);
        let mut changed = false;
        for (p, len) in files {
            let off = self.offsets.get(&p).copied().unwrap_or(0);
            // 文件变短：被重写过，从头再读（seen 防止重复计数）
            let off = if len < off { 0 } else { off };
            if len == off {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&p) else { continue };
            let mut buf = Vec::with_capacity((len - off) as usize);
            if f.seek(SeekFrom::Start(off)).is_err() || f.take(len - off).read_to_end(&mut buf).is_err() {
                continue;
            }
            let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { continue };
            self.offsets.insert(p, off + end as u64 + 1);
            for line in buf[..end].split(|&b| b == b'\n') {
                changed |= self.line(line);
            }
        }
        changed
    }

    fn line(&mut self, line: &[u8]) -> bool {
        let Ok(s) = std::str::from_utf8(line) else { return false };
        // 先按字面筛掉绝大多数行，只解析回复行和标题行
        let reply = s.contains("\"usage\"") && s.contains("\"assistant\"");
        if !reply && !s.contains("-title\"") {
            return false;
        }
        let Some(j) = Json::parse(s) else { return false };
        let sid = j.get("sessionId").str().unwrap_or("").to_string();
        match j.get("type").str() {
            Some("custom-title") | Some("ai-title") => {
                // 用户自己起的名字优先，AI 起的不覆盖它
                let info = self.sessions.entry(sid).or_default();
                if let Some(t) = j.get("customTitle").str() {
                    (info.title, info.custom) = (Some(t.to_string()), true);
                } else if let (Some(t), false) = (j.get("aiTitle").str(), info.custom) {
                    info.title = Some(t.to_string());
                }
                true
            }
            Some("assistant") => {
                let m = j.get("message");
                if let Some(tool) = m.get("content").arr().iter().rev().find(|c| c.get("type").str() == Some("tool_use")) {
                    self.sessions.entry(sid).or_default().tool = Some(tool_brief(tool.get("name").str().unwrap_or("?"), tool.get("input")));
                }
                let model = m.get("model").str().unwrap_or("");
                let (Some(id), Some(ts)) = (m.get("id").str(), j.get("timestamp").str().and_then(time::parse_iso)) else { return false };
                if model.starts_with('<') {
                    return false;
                }
                let mut h = DefaultHasher::new();
                id.hash(&mut h);
                if !self.seen.insert(h.finish()) {
                    return false;
                }
                let u = m.get("usage");
                let tok = [u.get("input_tokens").u64(), u.get("output_tokens").u64(), u.get("cache_creation_input_tokens").u64(), u.get("cache_read_input_tokens").u64()];
                let cwd = j.get("cwd").str().unwrap_or("?");
                let project = match self.projects.get(cwd) {
                    Some(p) => p.clone(),
                    None => self.projects.entry(cwd.to_string()).or_insert_with(|| project_of(cwd)).clone(),
                };
                add(self.days.entry(time::local_day(ts)).or_default().entry((project, model.to_string())).or_default(), &tok);
                true
            }
            _ => false,
        }
    }
}

/// 两份汇总按项取较大值
pub fn merge_max(into: &mut Days, from: &Days) {
    for (d, m) in from {
        let e = into.entry(*d).or_default();
        for (k, t) in m {
            let v = e.entry(k.clone()).or_default();
            for i in 0..4 {
                v[i] = v[i].max(t[i]);
            }
        }
    }
}

/// history.txt：每行 天号|项目|模型|输入|输出|缓存写|缓存读
pub fn to_text(days: &Days) -> String {
    let mut s = String::new();
    for (d, m) in days {
        let mut rows: Vec<_> = m.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        for ((p, md), t) in rows {
            s += &format!("{d}|{}|{md}|{}|{}|{}|{}\n", p.replace('|', "_"), t[0], t[1], t[2], t[3]);
        }
    }
    s
}

pub fn from_text(s: &str) -> Days {
    let mut days = Days::new();
    for l in s.lines() {
        let f: Vec<&str> = l.split('|').collect();
        if f.len() != 7 {
            continue;
        }
        let (Ok(d), Ok(a), Ok(b), Ok(c), Ok(e)) = (f[0].parse(), f[3].parse(), f[4].parse(), f[5].parse(), f[6].parse()) else { continue };
        days.entry(d).or_default().insert((f[1].to_string(), f[2].to_string()), [a, b, c, e]);
    }
    days
}

/// 一天的合计
pub fn day_total(days: &Days, d: i32, cache_read: bool) -> u64 {
    days.get(&d).map_or(0, |m| m.values().map(|t| total(t, cache_read)).sum())
}

/// [from, to] 这几天按项目、按模型的合计，各自从大到小
pub fn breakdown(days: &Days, from: i32, to: i32, cache_read: bool) -> (u64, Vec<(String, u64)>, Vec<(String, u64)>) {
    let (mut sum, mut proj, mut model) = (0, HashMap::<&str, u64>::new(), HashMap::<&str, u64>::new());
    for (_, m) in days.range(from..=to) {
        for ((p, md), t) in m {
            let n = total(t, cache_read);
            sum += n;
            *proj.entry(p).or_default() += n;
            *model.entry(md).or_default() += n;
        }
    }
    let sorted = |m: HashMap<&str, u64>| {
        let mut v: Vec<(String, u64)> = m.into_iter().filter(|e| e.1 > 0).map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    };
    (sum, sorted(proj), sorted(model))
}

/// 1234 → "1.2K"，12345678 → "12.3M"
pub fn fmt_tokens(n: u64) -> String {
    let f = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}K", f / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1}M", f / 1e6),
        _ => format!("{:.2}B", f / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(id: &str, ts: &str, out: u64, tool: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"s1","cwd":"C:\\W\\Proj","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"tool_use","name":"{tool}","input":{{"command":"cargo build"}}}}],"usage":{{"input_tokens":10,"output_tokens":{out},"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}}}}"#
        )
    }

    #[test]
    fn scan_dedup_incremental() {
        let dir = std::env::temp_dir().join(format!("ctile-test-{}", std::process::id()));
        let sub = dir.join("p").join("s1").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        let f = dir.join("p").join("s1.jsonl");
        // 同一条回复写了两行，只算一次；最后一行没换行，先不读
        let first = format!("{}\n{}\n{{\"type\":\"ai-title\",\"aiTitle\":\"AI 起的\",\"sessionId\":\"s1\"}}\n{}", reply("m1", "2026-10-17T12:00:00Z", 5, "Bash"), reply("m1", "2026-10-17T12:00:00Z", 5, "Bash"), reply("m2", "2026-10-17T12:01:00Z", 7, "Read"));
        std::fs::write(&f, &first).unwrap();
        std::fs::write(sub.join("a.jsonl"), reply("m3", "2026-10-17T12:02:00Z", 1, "Grep") + "\n").unwrap();
        let mut s = Scanner::new(dir.clone());
        assert!(s.scan());
        let d = time::local_day(time::parse_iso("2026-10-17T12:00:00Z").unwrap());
        let key = ("Proj".to_string(), "claude-opus-5-5".to_string());
        assert_eq!(s.days[&d][&key], [20, 6, 200, 2000]);
        assert_eq!(s.sessions["s1"].title.as_deref(), Some("AI 起的"));
        // 补上换行：m2 读进来；自定义标题盖过 AI 标题，之后的 AI 标题不再覆盖
        std::fs::write(&f, first + "\n{\"type\":\"custom-title\",\"customTitle\":\"我的\",\"sessionId\":\"s1\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"新\",\"sessionId\":\"s1\"}\n").unwrap();
        assert!(s.scan());
        assert_eq!(s.days[&d][&key], [30, 13, 300, 3000]);
        assert_eq!(s.sessions["s1"].title.as_deref(), Some("我的"));
        assert_eq!(s.sessions["s1"].tool.as_deref(), Some("Read: cargo build"));
        assert!(!s.scan());
        // 汇总存盘再读回来一致；取大值合并不重复计数
        let back = from_text(&to_text(&s.days));
        assert_eq!(back, s.days);
        let mut merged = back.clone();
        merge_max(&mut merged, &s.days);
        assert_eq!(merged, s.days);
        assert_eq!(day_total(&s.days, d, false), 343);
        let (sum, proj, model) = breakdown(&s.days, d, d, true);
        assert_eq!((sum, proj[0].0.as_str(), model[0].1), (3343, "Proj", 3343));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1234), "1.2K");
        assert_eq!(fmt_tokens(12_345_678), "12.3M");
        assert_eq!(base_name("C:\\a\\b\\"), "b");
        assert_eq!(tool_brief("Read", &Json::parse(r#"{"file_path":"C:\\x\\y.rs"}"#).unwrap()), "Read: y.rs");
    }
}
