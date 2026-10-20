//! VPS wake-up status/control over pinned SSH. Tokens stay on the VPS.
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

use crate::config::tr;
use crate::json::Json;
use crate::time;

pub const CONFIG: &str = "waker-ssh.json";

pub fn linked(data: &Path) -> bool {
    data.join(CONFIG).is_file()
}

#[derive(Debug)]
pub struct Status {
    pub configured: bool,
    pub enabled: bool,
    pub next: i64,
    pub result: String,
    pub at: Option<i64>,
    pub resets: Option<i64>,
    /// 已存 Token 的末 4 位，只用来认出存的是哪一个；旧版服务器不返回
    pub tail: String,
    /// 服务器上的 cron 文件在不在；旧版服务器不返回，按在算
    pub cron: bool,
}

impl Status {
    fn parse(text: &str) -> Option<Self> {
        let j = Json::parse(text)?;
        let (Json::Bool(configured), Json::Bool(enabled)) = (j.get("configured"), j.get("enabled")) else { return None };
        let state = j.get("state");
        Some(Self { configured: *configured, enabled: *enabled, next: j.get("next_at").num()? as i64,
            result: state.get("result").str().unwrap_or("").into(),
            at: state.get("at").num().map(|n| n as i64),
            resets: state.get("resets_at").num().map(|n| n as i64),
            tail: j.get("token_tail").str().unwrap_or("").into(),
            cron: !matches!(j.get("cron"), Json::Bool(false)) })
    }

    pub fn summary(&self, tz: u32) -> String {
        if !self.enabled {
            tr("已暂停", "Paused").into()
        } else if !self.configured {
            tr("等待填写 Token", "Token needed").into()
        } else if !self.cron {
            tr("定时任务未安装", "Schedule not installed").into()
        } else {
            format!("{} · {} {}", tr("已启用", "On"), tr("下次", "next"), time::stamp(self.next, tz))
        }
    }

    /// 第二行：VPS 上存了哪个 Token，以及最后一次执行的结果和时间
    pub fn detail(&self, tz: u32) -> String {
        let token = if !self.configured { tr("未存 Token", "No token saved").to_string() }
            else if self.tail.is_empty() { tr("Token 已保存", "Token saved").into() }
            else { format!("Token …{}", self.tail) };
        match self.at {
            Some(at) if !self.result.is_empty() => format!("{token} · {} {}", self.last(), time::stamp(at, tz)),
            _ => format!("{token} · {}", self.last()),
        }
    }

    pub fn last(&self) -> &'static str {
        match self.result.as_str() {
            "sent" => tr("唤醒请求成功", "Wake request succeeded"),
            "active_window" => tr("已有窗口，未重新唤醒", "Skipped: window already active"),
            "egress_mismatch" => tr("出口 IP 不符，已阻止请求", "Blocked: unexpected egress IP"),
            "proxy_unavailable" => tr("ISP 代理不可用", "ISP proxy unavailable"),
            "usage_http_401" => tr("Token 无效或已过期", "Token invalid or expired"),
            "" => tr("尚未执行", "Not run yet"),
            _ => tr("执行失败，等待下次计划", "Failed; awaiting next schedule"),
        }
    }
}

#[derive(Debug)]
pub struct Identity {
    pub result: String,
    pub email: String,
    pub plan: String,
    pub quota: Option<Vec<crate::quota::Window>>,
    pub usage_status: String,
}

impl Identity {
    pub fn has_usage(&self) -> bool {
        self.usage_status == "200" && self.quota.as_ref().is_some_and(|ws| ws.iter().any(|w|
            matches!(w.kind, crate::quota::Kind::Session | crate::quota::Kind::Week)
                && w.pct.is_finite() && w.resets.is_some()))
    }

    fn parse(text: &str) -> Option<Self> {
        let j = Json::parse(text)?;
        Some(Self { result: j.get("result").str()?.into(), email: j.get("email").str().unwrap_or("").into(), plan: j.get("plan").str().unwrap_or("").into(),
            quota: crate::quota::parse_value(j.get("usage")), usage_status: j.get("usage_status").str().unwrap_or("").into() })
    }

    pub fn note(&self) -> &'static str {
        match self.result.as_str() {
            "ok" => tr("由该 Token 的账号接口读取", "Read from this token's account profile"),
            "waiting_token" => tr("请在顶部唤醒行填写 Token", "Set a token using the Wake-up row"),
            "no_profile_scope" => tr("Token 无账号资料权限，邮箱和等级未获取", "Token lacks profile access; identity unavailable"),
            "expired" => tr("Token 无效或已过期", "Token invalid or expired"),
            "egress_mismatch" => tr("出口 IP 不符，已阻止账号查询", "Unexpected egress; profile query blocked"),
            _ => tr("账号资料暂时无法获取，请稍后刷新", "Profile unavailable; refresh later"),
        }
    }
}

fn ssh(data: &Path, action: &str) -> Result<Command, String> {
    let err = || tr("请检查 waker-ssh.json 的 VPS 和 SSH 密钥配置", "Check VPS and SSH keys in waker-ssh.json").to_string();
    let text = std::fs::read_to_string(data.join(CONFIG)).map_err(|_| err())?;
    let j = Json::parse(&text).ok_or_else(err)?;
    let host = j.get("host").str().ok_or_else(err)?;
    let user = j.get("user").str().ok_or_else(err)?;
    let valid = |s: &str| !s.is_empty() && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !valid(host) || !valid(user) { return Err(err()) }
    let identity = j.get("identity_file").str().ok_or_else(err)?;
    let known = j.get("known_hosts_file").str().ok_or_else(err)?;
    if !Path::new(identity).is_file() || !Path::new(known).is_file() { return Err(err()) }
    let mut c = Command::new("ssh.exe");
    // Ignore user's global SSH configuration: this connection uses only its
    // dedicated key, known-hosts file and restricted server-side command.
    c.args(["-F", "NUL", "-i", identity, "-o", &format!("UserKnownHostsFile={known}"),
        "-o", "StrictHostKeyChecking=yes", "-o", "IdentitiesOnly=yes",
        "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "ServerAliveInterval=5",
        "-o", "ServerAliveCountMax=1", if action == "configure" { "-tt" } else { "-T" },
        &format!("{user}@{host}"), action]);
    Ok(c)
}

pub fn fetch(data: &Path) -> Result<Status, String> {
    Status::parse(&request(data, "status", 20)?).ok_or_else(|| tr("VPS 状态格式不正确", "Invalid VPS status format").into())
}

pub fn identity(data: &Path) -> Result<Identity, String> {
    Identity::parse(&request(data, "profile", 90)?).ok_or_else(|| tr("VPS 账号资料格式不正确", "Invalid VPS profile format").into())
}

fn request(data: &Path, action: &str, timeout: u64) -> Result<String, String> {
    let unavailable = || tr("VPS 暂时无法连接（不影响服务器定时任务）", "VPS unreachable; its schedule runs independently").to_string();
    let mut command = ssh(data, action)?;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0).spawn().map_err(|_| unavailable())?;
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable());
            }
        }
    }
    let output = child.wait_with_output().map_err(|_| unavailable())?;
    if !output.status.success() { return Err(unavailable()) }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn configure(data: &Path) -> Result<(), String> {
    ssh(data, "configure")?.creation_flags(CREATE_NEW_CONSOLE.0).spawn()
        .map(|_| ()).map_err(|_| tr("无法打开 SSH 设置窗口", "Cannot open SSH settings").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_distinguishes_sent_from_confirmed_reset() {
        let s = Status::parse(r#"{"configured":true,"enabled":true,"next_at":1792537200,"state":{"result":"sent","at":1792450801,"resets_at":null}}"#).unwrap();
        assert_eq!(s.result, "sent");
        assert_eq!(s.resets, None);
        assert!(Status::parse(r#"{"error":"unauthorized"}"#).is_none());
        assert!(Status::parse(r#"{"configured":"true","enabled":true,"next_at":1}"#).is_none());
        // 旧版服务器没有 token_tail / cron
        assert_eq!(s.detail(0), "Token 已保存 · 唤醒请求成功 10-20 07:00");
        assert_eq!(s.summary(0), "已启用 · 下次 10-21 07:00");
        let s = Status::parse(r#"{"configured":true,"enabled":true,"next_at":1,"token_tail":"Ab12","cron":false,"state":{}}"#).unwrap();
        assert_eq!(s.detail(0), "Token …Ab12 · 尚未执行");
        assert_eq!(s.summary(0), "定时任务未安装");
    }

    #[test]
    fn identity_does_not_substitute_a_local_account() {
        let p = Identity::parse(r#"{"result":"no_profile_scope"}"#).unwrap();
        assert!(p.email.is_empty() && p.plan.is_empty());
        assert!(!p.has_usage());
        let p = Identity::parse(r#"{"result":"ok","email":"test@example.com","plan":"Max 5x"}"#).unwrap();
        assert_eq!(p.email, "test@example.com");
        assert_eq!(p.plan, "Max 5x");
        assert!(!p.has_usage());
    }

    #[test]
    fn account_card_requires_usage_and_reset_time() {
        let json = r#"{"result":"ok","usage_status":"200","usage":{"five_hour":{"utilization":0,"resets_at":"2026-10-20T04:00:00Z"}}}"#;
        assert!(Identity::parse(json).unwrap().has_usage());
        assert!(!Identity::parse(&json.replace("\"200\"", "\"403\"")).unwrap().has_usage());
        assert!(!Identity::parse(&json.replace("\"2026-10-20T04:00:00Z\"", "null")).unwrap().has_usage());
    }
}
