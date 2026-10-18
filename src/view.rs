//! 磁贴的排版和绘制。尺寸都是 DIP，绘制时由渲染目标按窗口 DPI 缩放。
//! 自上而下：标题行、额度、会话、Token 热力图、选中日 / 周的分项。高度随内容变，宽度可拖，热力图的周数跟着宽度走。

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::config::{en, tr};
use crate::quota::{self, Fetch, Kind};
use crate::sessions::State;
use crate::usage::{self, fmt_tokens};
use crate::{App, time};

pub const PAD: f32 = 16.0;
const GAP: f32 = 14.0;
const CELL: f32 = 11.0;
const STEP: f32 = 14.0;
const DAY_LABEL_W: f32 = 30.0;
pub const MIN_W: f32 = 300.0;
/// 框与框之间的空隙（也是拖动改框高的把手）、最下方留白、框最矮多高
const SPLIT: f32 = 16.0;
const BOTTOM: f32 = 12.0;
pub const MIN_BOX: f32 = 40.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Session(u32),
    Day(i32),
    /// 按周显示时的一周，值是周一的天号
    Week(i32),
    Toggle(bool),
    Quota,
    /// 右上角的置顶按钮
    Pin,
    /// 第 i 个框下方的拖动条
    Split(usize),
}

pub struct Theme {
    pub dark: bool,
    pub tint: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub muted: D2D1_COLOR_F,
    pub faint: D2D1_COLOR_F,
    pub track: D2D1_COLOR_F,
    pub hover: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub warn: D2D1_COLOR_F,
    pub danger: D2D1_COLOR_F,
    pub ok: D2D1_COLOR_F,
    /// 底色太薄时文字压在壁纸上，加阴影（和桌面图标一样）
    pub shadow: bool,
}

pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub fn with_a(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { a: c.a * a, ..c }
}

impl Theme {
    /// 磁贴的配色；opacity 是底色的不透明度（0.2～1）
    pub fn new(dark: bool, opacity: f32) -> Theme {
        let (bg, fg) = if dark { (0x20 as f32 / 255.0, 0xF3 as f32 / 255.0) } else { (0xF3 as f32 / 255.0, 0x1A as f32 / 255.0) };
        let g = |a: f32| rgba(fg, fg, fg, a);
        Theme {
            dark,
            tint: rgba(bg, bg, bg, opacity * if dark { 0.80 } else { 0.88 }),
            shadow: opacity < 0.5,
            text: g(1.0),
            muted: g(if dark { 0.66 } else { 0.64 }),
            faint: g(if dark { 0.09 } else { 0.08 }),
            track: g(if dark { 0.14 } else { 0.12 }),
            hover: g(0.07),
            // Claude 的陶土橙；浅色背景上压暗一点保证对比度
            accent: if dark { rgba(0.851, 0.467, 0.341, 1.0) } else { rgba(0.776, 0.380, 0.255, 1.0) },
            warn: if dark { rgba(0.957, 0.702, 0.251, 1.0) } else { rgba(0.722, 0.475, 0.0, 1.0) },
            danger: if dark { rgba(0.957, 0.392, 0.341, 1.0) } else { rgba(0.769, 0.169, 0.110, 1.0) },
            ok: if dark { rgba(0.412, 0.812, 0.533, 1.0) } else { rgba(0.110, 0.553, 0.255, 1.0) },
        }
    }

    /// 右键菜单和右下角提示用的不透明面板：不跟磁贴的透明度走
    pub fn panel(dark: bool) -> Theme {
        let bg = if dark { 0x2C as f32 / 255.0 } else { 0xF9 as f32 / 255.0 };
        Theme { tint: rgba(bg, bg, bg, 1.0), ..Theme::new(dark, 1.0) }
    }

    fn level(&self, pct: f64) -> D2D1_COLOR_F {
        if pct >= 90.0 {
            self.danger
        } else if pct >= 70.0 {
            self.warn
        } else {
            self.accent
        }
    }
}

pub struct Fonts {
    pub f11: IDWriteTextFormat,
    pub f12: IDWriteTextFormat,
    pub f12b: IDWriteTextFormat,
    pub f14b: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
    /// 12 号、可换行、顶对齐，提示卡片的正文用
    pub f12w: IDWriteTextFormat,
}

impl Fonts {
    pub fn new(dw: &IDWriteFactory) -> Fonts {
        let mk = |size: f32, weight: DWRITE_FONT_WEIGHT| unsafe {
            let f = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("zh-CN")).expect("创建字体失败");
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            if let Ok(sign) = dw.CreateEllipsisTrimmingSign(&f) {
                let _ = f.SetTrimming(&trim, &sign);
            }
            f
        };
        // Win11 的图标字体是 Segoe Fluent Icons，Win10 只有 Segoe MDL2 Assets，两者用到的码位相同
        let icon = unsafe {
            let mut coll = None;
            let _ = dw.GetSystemFontCollection(&mut coll, false);
            let (mut idx, mut found) = (0u32, BOOL(0));
            if let Some(c) = coll {
                let _ = c.FindFamilyName(w!("Segoe Fluent Icons"), &mut idx, &mut found);
            }
            let fam = if found.as_bool() { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let f = dw.CreateTextFormat(fam, None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 14.0, w!("en-US")).expect("创建字体失败");
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            f
        };
        let f12w = mk(12.0, DWRITE_FONT_WEIGHT_NORMAL);
        unsafe {
            let _ = f12w.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = f12w.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        Fonts { f11: mk(11.0, DWRITE_FONT_WEIGHT_NORMAL), f12: mk(12.0, DWRITE_FONT_WEIGHT_NORMAL), f12b: mk(12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), f14b: mk(14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), icon, f12w }
    }
}

pub fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

pub fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 2 小时 13 分 / 45 分 / 不到 1 分
pub fn fmt_dur(secs: i64) -> String {
    let m = secs.max(0) / 60;
    let (d, h, mm) = (m / 1440, m / 60 % 24, m % 60);
    match (en(), d, h) {
        (false, 0, 0) if m == 0 => "不到 1 分".into(),
        (false, 0, 0) => format!("{mm} 分"),
        (false, 0, _) => format!("{h} 小时 {mm} 分"),
        (false, _, _) => format!("{d} 天 {h} 小时"),
        (true, 0, 0) if m == 0 => "<1m".into(),
        (true, 0, 0) => format!("{mm}m"),
        (true, 0, _) => format!("{h}h {mm}m"),
        (true, _, _) => format!("{d}d {h}h"),
    }
}

pub fn fmt_date(day: i32) -> String {
    let (_, m, d) = time::civil(day);
    if en() {
        const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {d}", M[m as usize - 1])
    } else {
        format!("{m} 月 {d} 日")
    }
}

fn weekday_name(day: i32) -> &'static str {
    const ZH: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    const EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    (if en() { EN } else { ZH })[time::weekday(day) as usize]
}

pub fn monday(day: i32) -> i32 {
    day - ((time::weekday(day) + 6) % 7) as i32
}

pub fn quota_label(k: &Kind) -> String {
    match k {
        Kind::Session => tr("5 小时", "5-hour").into(),
        Kind::Week => tr("本周", "Weekly").into(),
        Kind::WeekModel(m) => format!("{} · {m}", tr("本周", "Weekly")),
    }
}

pub fn waiting_text(w: &str) -> &'static str {
    match w {
        "dialog open" => tr("等你确认", "Needs approval"),
        "input needed" => tr("等你回复", "Needs input"),
        "sandbox request" => tr("等你批准沙箱请求", "Sandbox request"),
        "goal proposal" => tr("等你确认目标", "Goal proposal"),
        _ => tr("等你处理", "Waiting for you"),
    }
}

/// 热力图分档阈值：非零值的四分位，和 GitHub 一样分 4 档
pub fn thresholds(vals: &[u64]) -> [u64; 3] {
    let mut v: Vec<u64> = vals.iter().copied().filter(|&x| x > 0).collect();
    if v.is_empty() {
        return [0; 3];
    }
    v.sort_unstable();
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    [q(0.25), q(0.5), q(0.75)]
}

pub fn level(v: u64, th: &[u64; 3]) -> usize {
    if v == 0 { 0 } else { 1 + th.iter().filter(|&&t| v > t).count() }
}

pub struct P<'a> {
    pub dc: &'a ID2D1DeviceContext,
    pub br: &'a ID2D1SolidColorBrush,
    pub dw: &'a IDWriteFactory,
    /// 文字加阴影
    pub shadow: bool,
}

#[derive(Clone, Copy)]
pub enum Align {
    L,
    C,
    R,
}

impl P<'_> {
    pub fn color(&self, c: D2D1_COLOR_F) {
        unsafe { self.br.SetColor(&c) };
    }

    pub fn fill(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F) {
        self.color(c);
        unsafe { self.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, self.br) };
    }

    pub fn dot(&self, x: f32, y: f32, r: f32, c: D2D1_COLOR_F) {
        self.color(c);
        unsafe { self.dc.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r }, self.br) };
    }

    /// 在 r 里画一行字，返回文字实际宽度
    pub fn text(&self, s: &str, r: D2D_RECT_F, f: &IDWriteTextFormat, c: D2D1_COLOR_F, al: Align) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = self.dw.CreateTextLayout(&w, f, (r.right - r.left).max(1.0), (r.bottom - r.top).max(1.0)) else { return 0.0 };
            let _ = l.SetTextAlignment(match al {
                Align::L => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::C => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::R => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            if self.shadow {
                self.color(rgba(0.0, 0.0, 0.0, 0.55 * c.a));
                self.dc.DrawTextLayout(Vector2 { X: r.left + 1.0, Y: r.top + 1.0 }, &l, self.br, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            self.color(c);
            self.dc.DrawTextLayout(Vector2 { X: r.left, Y: r.top }, &l, self.br, D2D1_DRAW_TEXT_OPTIONS_NONE);
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            m.width
        }
    }
}

/// 一帧的排版结果
pub struct Frame {
    pub h: f32,
    pub hits: Vec<(D2D_RECT_F, Hit)>,
    /// 三个框内容的自然高度，下一帧据此定框高
    pub nat: [f32; 3],
    /// 三个框实际的 (顶, 高)
    pub boxes: [(f32, f32); 3],
}

/// 框高：用户拖过的高度（不超过内容），没拖过就是内容高度；放不下屏幕（max_h）时先压会话框，再压 Token 框
pub fn box_heights(user: [f32; 3], nat: [f32; 3], fixed: f32, max_h: f32) -> [f32; 3] {
    let mut b = [0.0; 3];
    for i in 0..3 {
        b[i] = if user[i] > 0.0 { user[i].min(nat[i]) } else { nat[i] };
        b[i] = b[i].max(MIN_BOX.min(nat[i]));
    }
    let mut over = fixed + b.iter().sum::<f32>() - max_h;
    for i in [1, 2] {
        if over > 0.0 {
            let cut = over.min((b[i] - MIN_BOX.min(nat[i])).max(0.0));
            b[i] -= cut;
            over -= cut;
        }
    }
    b
}

/// 画整块磁贴
pub fn draw(a: &App, w: f32, h: f32, max_h: f32) -> Frame {
    let p = P { dc: &a.gfx.dc, br: &a.gfx.brush, dw: &a.dw, shadow: a.theme.shadow };
    let (th, f) = (&a.theme, &a.fonts);
    let mut hits = Vec::new();
    let now = time::now();
    let (l, r) = (PAD, w - PAD);
    unsafe {
        p.dc.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
        p.fill(rect(0.0, 0.0, w, h), 0.0, th.tint);
    }

    // 标题行：名字、套餐、更新状态、右上角的置顶按钮
    let mut y = 12.0;
    let tw = p.text("Claude", rect(l, y, r, y + 24.0), &f.f14b, th.text, Align::L);
    if !a.plan.is_empty() {
        p.text(&a.plan, rect(l + tw + 8.0, y + 1.0, r, y + 24.0), &f.f11, th.muted, Align::L);
    }
    let pin = rect(r - 24.0, y, r + 4.0, y + 24.0);
    if a.hover == Some(Hit::Pin) {
        p.fill(pin, 5.0, th.hover);
    }
    // Segoe Fluent Icons：E718 图钉，E840 按下的图钉
    let (glyph, gc) = if a.cfg.topmost { ("\u{E840}", th.accent) } else { ("\u{E718}", th.muted) };
    p.text(glyph, pin, &f.icon, gc, Align::C);
    hits.push((pin, Hit::Pin));
    let status = match &a.quota {
        _ if a.quota_at == 0 => tr("正在读取额度…", "Loading usage…").to_string(),
        Fetch::Ok(_) => {
            let ago = now - a.quota_at;
            let s = if ago < 60 { tr("刚刚更新", "Updated just now").to_string() } else if en() { format!("Updated {} ago", fmt_dur(ago)) } else { format!("{}前更新", fmt_dur(ago)) };
            if a.stale { format!("{s} · {}", tr("刷新失败", "refresh failed")) } else { s }
        }
        Fetch::Expired => tr("登录已过期", "Login expired").to_string(),
        Fetch::NoLogin => tr("未登录", "Not logged in").to_string(),
        Fetch::Changed(_) => tr("需要更新 ClaudeTile", "ClaudeTile needs an update").to_string(),
        Fetch::Err(e) => format!("{}{e}", tr("额度读取失败：", "Usage error: ")),
    };
    let stale = a.stale || !matches!(a.quota, Fetch::Ok(_)) && a.quota_at != 0;
    p.text(&status, rect(l + 120.0, y + 1.0, r - 30.0, y + 24.0), &f.f11, if stale { th.warn } else { th.muted }, Align::R);
    y += 24.0 + GAP;

    // 三个框：额度、会话、Token。各自裁剪、可滚动，框下方的空隙可以拖动改框高
    let fixed = y + 2.0 * SPLIT + BOTTOM;
    let bh = box_heights(a.cfg.heights, a.nat, fixed, max_h);
    let mut nat = [0.0; 3];
    let mut boxes = [(0.0, 0.0); 3];
    for i in 0..3 {
        let (top, hgt) = (y, bh[i]);
        let scroll = a.scroll[i].min((a.nat[i] - hgt).max(0.0)).max(0.0);
        // 会话框的标题固定在框顶，下面的列表滚动
        let head = if i == 1 { session_head(&p, a, w, top) } else { 0.0 };
        let (ctop, vis) = (top + head, (hgt - head).max(0.0));
        let mut local = Vec::new();
        unsafe {
            p.dc.PushAxisAlignedClip(&rect(0.0, ctop, w, top + hgt), D2D1_ANTIALIAS_MODE_ALIASED);
            p.dc.SetTransform(&Matrix3x2::translation(0.0, ctop - scroll));
            nat[i] = head
                + match i {
                    0 => quota_box(&p, a, w, now, &mut local),
                    1 => session_box(&p, a, w, now, &mut local),
                    _ => token_box(&p, a, w, &mut local),
                };
            p.dc.SetTransform(&Matrix3x2::identity());
            p.dc.PopAxisAlignedClip();
        }
        for (rc, hit) in local {
            let rc = rect(rc.left, rc.top + ctop - scroll, rc.right, rc.bottom + ctop - scroll);
            if rc.bottom > ctop && rc.top < top + hgt {
                hits.push((rect(rc.left, rc.top.max(ctop), rc.right, rc.bottom.min(top + hgt)), hit));
            }
        }
        // 内容比框高时右边画一条细滚动条
        let content = a.nat[i] - head;
        if content > vis + 0.5 && vis > 0.0 {
            let thumb = (vis * vis / content).max(16.0);
            let ty = ctop + (vis - thumb) * scroll / (content - vis);
            p.fill(rect(w - 6.0, ty, w - 3.0, ty + thumb), 1.5, th.track);
        }
        boxes[i] = (top, hgt);
        y = top + hgt;
        // 框下方的拖动条：悬停或拖动时显出一道短横线
        let gap = if i < 2 { SPLIT } else { BOTTOM };
        let zone = rect(0.0, y, w, y + gap);
        if a.hover == Some(Hit::Split(i)) || a.dragging == Some(i) {
            let c = w / 2.0;
            p.fill(rect(c - 18.0, y + gap / 2.0 - 1.5, c + 18.0, y + gap / 2.0 + 1.5), 1.5, th.muted);
        }
        hits.push((zone, Hit::Split(i)));
        y += gap;
    }
    Frame { h: y, hits, nat, boxes }
}

fn quota_box(p: &P, a: &App, w: f32, now: i64, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let mut y = 0.0;
    let note = |msg: &str, y: &mut f32| {
        p.text(msg, rect(l, *y, r, *y + 18.0), &f.f12, th.muted, Align::L);
        *y += 18.0;
    };
    match &a.quota {
        Fetch::Ok(ws) => {
            for wd in ws {
                let pct = wd.pct.clamp(0.0, 100.0);
                p.text(&quota_label(&wd.kind), rect(l, y, r, y + 16.0), &f.f12, th.text, Align::L);
                let reset = wd.resets.map(|t| if en() { format!("resets in {}", fmt_dur(t - now)) } else { format!("{}后重置", fmt_dur(t - now)) }).unwrap_or_default();
                let rw = p.text(&reset, rect(l, y, r, y + 16.0), &f.f11, th.muted, Align::R);
                p.text(&format!("{pct:.0}%"), rect(l, y, r - rw - 8.0, y + 16.0), &f.f12b, th.level(pct), Align::R);
                y += 20.0;
                p.fill(rect(l, y, r, y + 6.0), 3.0, th.track);
                if pct > 0.0 {
                    p.fill(rect(l, y, l + ((r - l) * pct as f32 / 100.0).max(6.0), y + 6.0), 3.0, th.level(pct));
                }
                y += 6.0;
                if wd.kind == Kind::Session {
                    if let Some(t) = quota::eta(&a.samples, now).filter(|t| wd.resets.is_none_or(|rs| *t < rs)) {
                        y += 4.0;
                        let s = if en() { format!("At this pace, runs out in {}", fmt_dur(t - now)) } else { format!("按当前速度，约 {} 后用完", fmt_dur(t - now)) };
                        p.text(&s, rect(l, y, r, y + 16.0), &f.f11, th.warn, Align::L);
                        y += 16.0;
                    }
                }
                y += 10.0;
            }
            y -= 10.0;
        }
        Fetch::Expired => note(tr("登录已过期：打开一次 Claude Code 即可自动刷新", "Login expired: open Claude Code once to refresh"), &mut y),
        Fetch::NoLogin => note(tr("没有找到 Claude 登录信息（用 API Key 时没有额度）", "No Claude login found (API keys have no plan limits)"), &mut y),
        Fetch::Changed(_) => {
            p.text(tr("额度接口有变化，当前版本读不了额度", "The usage API has changed and this version can't read it"), rect(l, y, r, y + 18.0), &f.f12, th.warn, Align::L);
            y += 18.0;
            note(tr("请更新 ClaudeTile（会话和 Token 统计不受影响）", "Please update ClaudeTile (sessions and tokens still work)"), &mut y);
        }
        Fetch::Err(_) => note(tr("额度暂时读不到，稍后自动重试（点这里立即重试）", "Usage unavailable, retrying soon (click to retry)"), &mut y),
    }
    hits.push((rect(0.0, -4.0, w, y + 4.0), Hit::Quota));
    y
}

/// 会话框的标题行，画在框顶不滚动，返回高度
fn session_head(p: &P, a: &App, w: f32, y: f32) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let busy = a.sessions.iter().filter(|s| s.state != State::Idle).count();
    let tw = p.text(tr("会话", "Sessions"), rect(l, y, r, y + 18.0), &f.f12b, th.text, Align::L);
    if !a.sessions.is_empty() {
        let n = if en() { format!("{busy} active / {}", a.sessions.len()) } else { format!("{busy} 个进行中 / 共 {}", a.sessions.len()) };
        p.text(&n, rect(l + tw + 8.0, y + 1.0, r, y + 18.0), &f.f11, th.muted, Align::L);
    }
    22.0
}

fn session_box(p: &P, a: &App, w: f32, now: i64, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let mut y = 0.0;
    if a.sessions.is_empty() {
        p.text(tr("没有运行中的会话", "No running sessions"), rect(l, y, r, y + 18.0), &f.f12, th.muted, Align::L);
        y += 18.0;
    }
    for s in &a.sessions {
        let row = rect(l - 8.0, y, r + 8.0, y + 40.0);
        if a.hover == Some(Hit::Session(s.pid)) {
            p.fill(row, 6.0, th.hover);
        }
        hits.push((row, Hit::Session(s.pid)));
        let info = a.info.get(&s.id);
        let title = a.desk_titles.get(&s.id).cloned().or_else(|| info.and_then(|i| i.title.clone())).unwrap_or_else(|| s.name.clone());
        let (color, state) = match &s.state {
            State::Waiting(wf) => (th.warn, waiting_text(wf).to_string()),
            State::Busy => (th.ok, if en() { format!("Running · {}", fmt_dur(now - s.since)) } else { format!("运行中 · {}", fmt_dur(now - s.since)) }),
            State::Idle => (th.muted, if en() { format!("Idle · {}", fmt_dur(now - s.since)) } else { format!("空闲 · {}", fmt_dur(now - s.since)) }),
        };
        p.dot(l + 4.0, y + 11.0, 4.0, color);
        let idle = s.state == State::Idle;
        let sw = p.text(&state, rect(l + 120.0, y + 2.0, r, y + 20.0), &f.f11, if idle { th.muted } else { color }, Align::R);
        p.text(&title, rect(l + 16.0, y + 2.0, r - sw - 10.0, y + 20.0), &f.f12b, if idle { th.muted } else { th.text }, Align::L);
        let mut sub = String::new();
        if s.desktop {
            sub += tr("桌面版 · ", "Desktop · ");
        }
        sub += &s.project;
        if let (false, Some(t)) = (idle, info.and_then(|i| i.tool.as_ref())) {
            sub += " · ";
            sub += t;
        }
        p.text(&sub, rect(l + 16.0, y + 20.0, r, y + 38.0), &f.f11, th.muted, Align::L);
        y += 40.0;
    }
    y
}

fn token_box(p: &P, a: &App, w: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let mut y = 0.0;
    let today = time::today();
    let weeks = (((r - l - DAY_LABEL_W) + (STEP - CELL)) / STEP).floor().max(4.0) as i32;
    let start = monday(today) - (weeks - 1) * 7;
    let cr = a.cfg.cache_read;
    let tw = p.text("Token", rect(l, y, r, y + 20.0), &f.f12b, th.text, Align::L);
    let sum: u64 = (start..=today).map(|d| usage::day_total(&a.days, d, cr)).sum();
    let span = if en() { format!("{weeks} weeks · {}", fmt_tokens(sum)) } else { format!("近 {weeks} 周 · {}", fmt_tokens(sum)) };
    p.text(&span, rect(l + tw + 8.0, y + 1.0, r - 70.0, y + 20.0), &f.f11, th.muted, Align::L);
    // 日 / 周切换
    for (i, (label, weekly)) in [(tr("日", "Day"), false), (tr("周", "Week"), true)].into_iter().enumerate() {
        let b = rect(r - 64.0 + i as f32 * 32.0, y, r - 32.0 + i as f32 * 32.0, y + 20.0);
        let on = a.cfg.weekly == weekly;
        p.fill(b, 5.0, if on { th.track } else if a.hover == Some(Hit::Toggle(weekly)) { th.hover } else { rgba(0.0, 0.0, 0.0, 0.0) });
        let tf = if on { &f.f12b } else { &f.f12 };
        p.text(label, b, tf, if on { th.text } else { th.muted }, Align::C);
        hits.push((b, Hit::Toggle(weekly)));
    }
    y += 26.0;

    let gx = l + DAY_LABEL_W;
    // 月份标签：某一列的周一换了月份就标一下，太挤就跳过
    let mut last_x = -100.0;
    let mut last_m = 0;
    for c in 0..weeks {
        let (_, m, _) = time::civil(start + c * 7);
        let x = gx + c as f32 * STEP;
        if m != last_m && x - last_x >= 30.0 {
            let ml = if en() { ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][m as usize - 1].to_string() } else { format!("{m}月") };
            p.text(&ml, rect(x, y, x + 40.0, y + 14.0), &f.f11, th.muted, Align::L);
            last_x = x;
        }
        last_m = m;
    }
    y += 16.0;
    let gy = y;
    let sel = a.sel;
    if !a.cfg.weekly {
        for (i, wd) in [(0, tr("一", "Mon")), (2, tr("三", "Wed")), (4, tr("五", "Fri"))] {
            p.text(wd, rect(l, gy + i as f32 * STEP - 2.0, gx, gy + i as f32 * STEP + CELL + 2.0), &f.f11, th.muted, Align::L);
        }
        let vals: Vec<u64> = (start..=today).map(|d| usage::day_total(&a.days, d, cr)).collect();
        let thr = thresholds(&vals);
        for c in 0..weeks {
            for rr in 0..7 {
                let d = start + c * 7 + rr;
                if d > today {
                    continue;
                }
                let v = vals[(d - start) as usize];
                let cell = rect(gx + c as f32 * STEP, gy + rr as f32 * STEP, gx + c as f32 * STEP + CELL, gy + rr as f32 * STEP + CELL);
                let lv = level(v, &thr);
                p.fill(cell, 2.0, if lv == 0 { th.faint } else { with_a(th.accent, [0.0, 0.32, 0.55, 0.78, 1.0][lv]) });
                if sel == Some(d) || a.hover == Some(Hit::Day(d)) || (sel.is_none() && d == today) {
                    p.color(th.text);
                    unsafe { a.gfx.dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(cell.left - 1.0, cell.top - 1.0, cell.right + 1.0, cell.bottom + 1.0), radiusX: 3.0, radiusY: 3.0 }, &a.gfx.brush, 1.0, None) };
                }
                hits.push((rect(cell.left - 1.5, cell.top - 1.5, cell.right + 1.5, cell.bottom + 1.5), Hit::Day(d)));
            }
        }
    } else {
        let vals: Vec<u64> = (0..weeks).map(|c| (0..7).map(|k| usage::day_total(&a.days, start + c * 7 + k, cr)).sum()).collect();
        let max = vals.iter().copied().max().unwrap_or(0).max(1);
        let hgt = 7.0 * STEP - (STEP - CELL);
        for (c, &v) in vals.iter().enumerate() {
            let x = gx + c as f32 * STEP;
            let wk = start + c as i32 * 7;
            let bh = if v == 0 { 2.0 } else { (hgt * v as f32 / max as f32).max(3.0) };
            let on = sel == Some(wk) || a.hover == Some(Hit::Week(wk)) || (sel.is_none() && c as i32 == weeks - 1);
            p.fill(rect(x, gy + hgt - bh, x + CELL, gy + hgt), 2.0, if v == 0 { th.faint } else if on { th.accent } else { with_a(th.accent, 0.55) });
            hits.push((rect(x - 1.5, gy, x + CELL + 1.5, gy + hgt), Hit::Week(wk)));
        }
        p.text(&fmt_tokens(max), rect(l - 4.0, gy - 2.0, gx, gy + 12.0), &f.f11, th.muted, Align::L);
    }
    y = gy + 7.0 * STEP + 4.0;

    // 悬停 / 选中的说明，右边图例
    let focus = match a.hover {
        Some(Hit::Day(d)) => Some((d, d)),
        Some(Hit::Week(d)) => Some((d, d + 6)),
        _ => None,
    }
    .or(sel.map(|d| if a.cfg.weekly { (d, d + 6) } else { (d, d) }))
    .unwrap_or(if a.cfg.weekly { (monday(today), today) } else { (today, today) });
    let ftot: u64 = (focus.0..=focus.1.min(today)).map(|d| usage::day_total(&a.days, d, cr)).sum();
    let ftxt = if focus.0 == focus.1 {
        format!("{} {} · {}", fmt_date(focus.0), weekday_name(focus.0), fmt_tokens(ftot))
    } else {
        format!("{} – {} · {}", fmt_date(focus.0), fmt_date(focus.1), fmt_tokens(ftot))
    };
    p.text(&ftxt, rect(l, y, r - 110.0, y + 18.0), &f.f11, th.text, Align::L);
    if !a.cfg.weekly {
        let mut x = r - 5.0 * STEP;
        p.text(tr("多", "More"), rect(r - 40.0, y, r, y + 18.0), &f.f11, th.muted, Align::R);
        x -= 26.0;
        p.text(tr("少", "Less"), rect(x - 40.0, y, x - 4.0, y + 18.0), &f.f11, th.muted, Align::R);
        for lv in 0..5 {
            let cx = x + lv as f32 * STEP;
            p.fill(rect(cx, y + 3.5, cx + CELL, y + 3.5 + CELL), 2.0, if lv == 0 { th.faint } else { with_a(th.accent, [0.0, 0.32, 0.55, 0.78, 1.0][lv]) });
        }
    }
    y += 18.0 + GAP;

    // 分项：选中（或当前）的日 / 周，按项目、按模型
    let (_, proj, model) = usage::breakdown(&a.days, focus.0, focus.1, cr);
    let head = if focus.0 == focus.1 && focus.0 == today {
        tr("今天", "Today").to_string()
    } else if focus.0 == monday(today) && focus.1 >= today {
        tr("本周", "This week").to_string()
    } else if focus.0 == focus.1 {
        fmt_date(focus.0)
    } else {
        format!("{} – {}", fmt_date(focus.0), fmt_date(focus.1))
    };
    let tw = p.text(&head, rect(l, y, r, y + 18.0), &f.f12b, th.text, Align::L);
    p.text(&fmt_tokens(ftot), rect(l + tw + 8.0, y + 1.0, r, y + 18.0), &f.f11, th.muted, Align::L);
    // 看的不是本周时，右边给出本周合计方便对比
    if focus.0 != monday(today) || focus.1 < today {
        let wk: u64 = (monday(today)..=today).map(|d| usage::day_total(&a.days, d, cr)).sum();
        p.text(&format!("{} {}", tr("本周", "This week"), fmt_tokens(wk)), rect(l, y + 1.0, r, y + 18.0), &f.f11, th.muted, Align::R);
    }
    y += 22.0;
    if proj.is_empty() {
        p.text(tr("这段时间没有用量", "No usage in this period"), rect(l, y, r, y + 18.0), &f.f11, th.muted, Align::L);
        y += 18.0;
    }
    let col = (r - l) * 0.38;
    for (list, n, model_names) in [(&proj, 4usize, false), (&model, 3usize, true)] {
        let max = list.first().map_or(1, |e| e.1).max(1);
        for (name, v) in list.iter().take(n) {
            let name = if model_names { usage::model_name(name) } else { name.clone() };
            p.text(&name, rect(l, y, l + col - 8.0, y + 18.0), &f.f11, th.text, Align::L);
            let vw = p.text(&fmt_tokens(*v), rect(l, y, r, y + 18.0), &f.f11, th.muted, Align::R);
            let bw = (r - vw - 8.0 - (l + col)).max(10.0);
            p.fill(rect(l + col, y + 6.0, l + col + bw * (*v as f32 / max as f32).max(0.02), y + 12.0), 3.0, if model_names { with_a(th.text, 0.35) } else { with_a(th.accent, 0.75) });
            y += 18.0;
        }
        if !list.is_empty() {
            y += 6.0;
        }
    }
    // 最后一组列表后面的间距不要
    if proj.is_empty() { y } else { y - 6.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_levels() {
        let th = thresholds(&[0, 10, 20, 30, 40, 50]);
        assert_eq!(th, [20, 30, 40]);
        assert_eq!([0, 5, 20, 25, 35, 99].map(|v| level(v, &th)), [0, 1, 1, 2, 3, 4]);
        assert_eq!(thresholds(&[0, 0]), [0; 3]);
        // 只有一个非零值时它是最深一档以下的 1 档，不会越界
        assert_eq!(level(7, &thresholds(&[7])), 1);
    }

    #[test]
    fn boxes() {
        let nat = [100.0, 400.0, 300.0];
        // 没拖过：内容多高框多高；拖过：不超过内容
        assert_eq!(box_heights([0.0; 3], nat, 50.0, 2000.0), nat);
        assert_eq!(box_heights([0.0, 200.0, 999.0], nat, 50.0, 2000.0), [100.0, 200.0, 300.0]);
        // 拖得比最矮还矮按最矮算；内容本身比最矮还矮就按内容
        assert_eq!(box_heights([10.0, 10.0, 0.0], [30.0, 400.0, 300.0], 50.0, 2000.0), [30.0, MIN_BOX, 300.0]);
        // 屏幕放不下：先压会话框，压到最矮再压 Token 框
        assert_eq!(box_heights([0.0; 3], nat, 50.0, 650.0), [100.0, 200.0, 300.0]);
        assert_eq!(box_heights([0.0; 3], nat, 50.0, 400.0), [100.0, MIN_BOX, 210.0]);
    }

    #[test]
    fn week_start() {
        // 2026-10-18 周日 → 周一是 10-12
        assert_eq!(monday(20744), 20738);
        assert_eq!(monday(20738), 20738);
    }
}
