//! 5 小时 / 每周额度：用 Claude Code 登录时存的 OAuth token（~/.claude/.credentials.json）调
//! api.anthropic.com/api/oauth/usage。这是非公开接口，字段可能变；拿不到就显示原因，不影响其他功能。
//!
//! token 过期由 Claude Code 自己刷新（它会改写凭证文件），这里每次请求前重新读文件，不自己刷新：
//! 刷新会换掉 refresh token，和正在运行的 Claude Code 抢着写凭证文件。
//!
//! 代理：先看环境变量，再看 ~/.claude/settings.json 的 env（Claude Code 走的就是它），都没有才用系统代理。

use std::path::Path;

use windows::Win32::Networking::WinHttp::*;
use windows::core::*;

use crate::json::Json;
use crate::time;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Session,
    Week,
    /// 按模型单独计的周额度，带模型名
    WeekModel(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub kind: Kind,
    pub pct: f64,
    pub resets: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fetch {
    Ok(Vec<Window>),
    /// 登录过期：打开一次 Claude Code 就会刷新
    Expired,
    /// 没登录（没有凭证文件），或用的是 API Key
    NoLogin,
    /// 接口变了（地址没了、要求变了、返回的格式认不出）：要更新本程序才能读
    Changed(String),
    /// 网络、限流、服务端出错这类临时问题，过会儿自动重试
    Err(String),
}

pub struct Creds {
    token: String,
    expires: i64,
    pub plan: String,
}

pub fn creds(claude_dir: &Path) -> Option<Creds> {
    let j = Json::parse(&std::fs::read_to_string(claude_dir.join(".credentials.json")).ok()?)?;
    let o = j.get("claudeAiOauth");
    Some(Creds { token: o.get("accessToken").str()?.to_string(), expires: (o.get("expiresAt").num().unwrap_or(0.0) / 1000.0) as i64, plan: plan_name(o.get("subscriptionType").str().unwrap_or(""), o.get("rateLimitTier").str().unwrap_or("")) })
}

/// "max" + "default_claude_max_5x" → "Max 5x"
pub fn plan_name(sub: &str, tier: &str) -> String {
    let mut s = sub.to_string();
    if let Some(c) = s.get_mut(..1) {
        c.make_ascii_uppercase();
    }
    match tier.rsplit('_').next().filter(|x| x.ends_with('x') && x[..x.len() - 1].chars().all(|c| c.is_ascii_digit()) && x.len() > 1) {
        Some(m) => format!("{s} {m}"),
        None => s,
    }
}

pub fn parse(s: &str) -> Option<Vec<Window>> {
    let j = Json::parse(s)?;
    parse_value(&j)
}

pub fn parse_value(j: &Json) -> Option<Vec<Window>> {
    let resets = |w: &Json| w.get("resets_at").str().and_then(time::parse_iso);
    let mut v = Vec::new();
    // limits 是新格式，带按模型计的周额度；没有时退回 five_hour / seven_day
    for l in j.get("limits").arr() {
        let kind = match l.get("kind").str() {
            Some("session") => Kind::Session,
            Some("weekly_all") => Kind::Week,
            Some("weekly_scoped") => match l.get("scope").get("model").get("display_name").str() {
                Some(m) => Kind::WeekModel(m.to_string()),
                None => continue,
            },
            _ => continue,
        };
        v.push(Window { kind, pct: l.get("percent").num().unwrap_or(0.0), resets: resets(l) });
    }
    if v.is_empty() {
        for (k, kind) in [("five_hour", Kind::Session), ("seven_day", Kind::Week), ("seven_day_opus", Kind::WeekModel("Opus".into())), ("seven_day_sonnet", Kind::WeekModel("Sonnet".into()))] {
            let w = j.get(k);
            if let Some(p) = w.get("utilization").num() {
                v.push(Window { kind, pct: p, resets: resets(w) });
            }
        }
    }
    (!v.is_empty()).then_some(v)
}

/// 原样返回（子进程的 HTTPS_PROXY 要带协议）；WinHTTP 用时再去掉协议
pub fn proxy(claude_dir: &Path) -> Option<String> {
    const KEYS: [&str; 6] = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy"];
    let from_env = KEYS.iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    let from_settings = || {
        let j = Json::parse(&std::fs::read_to_string(claude_dir.join("settings.json")).ok()?)?;
        KEYS.iter().find_map(|k| j.get("env").get(k).str().filter(|v| !v.is_empty()).map(str::to_string))
    };
    from_env.or_else(from_settings)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// 用 dir 里凭证的账号查额度。阻塞调用，在后台线程里跑
pub fn fetch(dir: &Path, proxy: Option<&str>, fake: Option<&Path>) -> Fetch {
    if let Some(f) = fake {
        // 测试用：文件读不到算临时失败，内容认不出算接口变了
        return match std::fs::read_to_string(f) {
            Ok(s) => parse(&s).map_or(Fetch::Changed("fake".into()), Fetch::Ok),
            Err(e) => Fetch::Err(e.to_string()),
        };
    }
    let Some(c) = creds(dir) else { return Fetch::NoLogin };
    if c.expires > 0 && c.expires < time::now() {
        return Fetch::Expired;
    }
    match get(&c.token, proxy) {
        Ok((200, body)) => parse(&body).map_or(Fetch::Changed("unrecognized response".into()), Fetch::Ok),
        Ok((401, _)) => Fetch::Expired,
        Ok((code @ (400 | 403 | 404 | 410), _)) => Fetch::Changed(format!("HTTP {code}")),
        Ok((code, _)) => Fetch::Err(format!("HTTP {code}")),
        Err(e) => Fetch::Err(e.message()),
    }
}

struct H(*mut core::ffi::c_void);

impl Drop for H {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn get(token: &str, proxy: Option<&str>) -> Result<(u32, String)> {
    http_get("api.anthropic.com", "/api/oauth/usage", &format!("Authorization: Bearer {token}\r\nanthropic-beta: oauth-2025-04-20\r\nAccept: application/json\r\n"), proxy)
}

/// Shared HTTPS transport. Public feeds pass only Accept, never account credentials.
pub fn http_get(host: &str, path: &str, headers: &str, proxy: Option<&str>) -> Result<(u32, String)> {
    unsafe {
        // WinHTTP 要 host:port，不带协议和末尾斜杠
        let p = proxy.map(|p| wide(p.split("://").last().unwrap_or(p).trim_end_matches('/')));
        let (access, pp) = match &p {
            Some(p) => (WINHTTP_ACCESS_TYPE_NAMED_PROXY, PCWSTR(p.as_ptr())),
            None => (WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null()),
        };
        let s = H(WinHttpOpen(w!("ClaudeTile"), access, pp, PCWSTR::null(), 0));
        if s.0.is_null() {
            return Err(Error::from_thread());
        }
        WinHttpSetTimeouts(s.0, 10_000, 10_000, 15_000, 15_000)?;
        let host = wide(host);
        let path = wide(path);
        let c = H(WinHttpConnect(s.0, PCWSTR(host.as_ptr()), 443, 0));
        if c.0.is_null() {
            return Err(Error::from_thread());
        }
        let r = H(WinHttpOpenRequest(c.0, w!("GET"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if r.0.is_null() {
            return Err(Error::from_thread());
        }
        let headers: Vec<u16> = headers.encode_utf16().collect();
        WinHttpSendRequest(r.0, Some(&headers), None, 0, 0, 0)?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut())?;
        let (mut code, mut len) = (0u32, 4u32);
        WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut code as *mut _ as _), &mut len, std::ptr::null_mut())?;
        let mut body = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut n = 0u32;
            WinHttpReadData(r.0, buf.as_mut_ptr() as _, buf.len() as u32, &mut n)?;
            if n == 0 {
                break;
            }
            if body.len() + n as usize > 1024 * 1024 {
                return Err(Error::new(windows::Win32::Foundation::E_FAIL, "Response too large"));
            }
            body.extend_from_slice(&buf[..n as usize]);
        }
        Ok((code, String::from_utf8_lossy(&body).into_owned()))
    }
}

/// 按最近 30 分钟的用量走势（最小二乘直线）估算 5 小时额度多久后用完，返回用完的时刻。
/// 样本跨度不到 5 分钟或没在涨时返回 None
pub fn eta(samples: &[(i64, f64)], now: i64) -> Option<i64> {
    let s: Vec<_> = samples.iter().filter(|(t, _)| now - t <= 1800).collect();
    let (t0, t1) = (s.first()?.0, s.last()?.0);
    if t1 - t0 < 300 {
        return None;
    }
    let n = s.len() as f64;
    let mx = s.iter().map(|(t, _)| (t - t0) as f64).sum::<f64>() / n;
    let my = s.iter().map(|(_, p)| p).sum::<f64>() / n;
    let sxy: f64 = s.iter().map(|(t, p)| ((t - t0) as f64 - mx) * (p - my)).sum();
    let sxx: f64 = s.iter().map(|(t, _)| ((t - t0) as f64 - mx).powi(2)).sum();
    let k = sxy / sxx;
    if !(k > 0.0) {
        return None;
    }
    // 从最新一个点按斜率外推
    let last = s.last()?;
    Some(last.0 + ((100.0 - last.1).max(0.0) / k) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_limits() {
        let s = r#"{"five_hour":{"utilization":12.0,"resets_at":"2026-10-18T18:39:59.955083+00:00"},"seven_day":{"utilization":16.0,"resets_at":null},
            "limits":[{"kind":"session","percent":12,"resets_at":"2026-10-18T18:39:59.955083+00:00"},{"kind":"weekly_all","percent":16,"resets_at":"2026-10-23T06:59:59+00:00"},
            {"kind":"weekly_scoped","percent":3,"resets_at":null,"scope":{"model":{"id":null,"display_name":"Fable"}}},{"kind":"other","percent":1}]}"#;
        let v = parse(s).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], Window { kind: Kind::Session, pct: 12.0, resets: time::parse_iso("2026-10-18T18:39:59Z") });
        assert_eq!(v[2].kind, Kind::WeekModel("Fable".into()));
        // 没有 limits 时用旧字段
        let old = parse(r#"{"five_hour":{"utilization":5,"resets_at":null},"seven_day":null}"#).unwrap();
        assert_eq!(old, vec![Window { kind: Kind::Session, pct: 5.0, resets: None }]);
        assert!(parse("{}").is_none());
        assert_eq!(plan_name("max", "default_claude_max_5x"), "Max 5x");
        assert_eq!(plan_name("pro", "default"), "Pro");
    }

    #[test]
    fn burn_eta() {
        // 每 2 分钟涨 1%，现在 50%：还要 100 分钟
        let s: Vec<(i64, f64)> = (0..10).map(|i| (i * 120, 41.0 + i as f64)).collect();
        assert_eq!(eta(&s, 1080), Some(1080 + 6000));
        // 跨度太短、不涨、太旧都不估
        assert_eq!(eta(&s[..2], 120), None);
        assert_eq!(eta(&[(0, 10.0), (600, 10.0)], 600), None);
        assert_eq!(eta(&s, 1080 + 3600), None);
    }
}
