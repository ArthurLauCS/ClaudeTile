//! 磁贴的排版和绘制（开发者终端风格）。尺寸都是 DIP，绘制时由渲染目标按窗口 DPI 缩放；颜色、字体、字号都取自 theme.rs。
//! 自上而下：标题、同步/唤醒、四个可滚动的框（账号额度、会话、Token、技能）、重置信息、状态栏。

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::config::{en, tr};
use crate::fontload;
use crate::quota::{self, Fetch, Kind};
use crate::sessions::State;
pub use crate::theme::Theme;
use crate::theme::{self, PAD, ROW, STATUS_H, with_a};
use crate::usage::{self, fmt_tokens};
use crate::{App, time};

const CELL: f32 = 11.0;
const STEP: f32 = 14.0;
const DAY_LABEL_W: f32 = 30.0;
pub const MIN_W: f32 = 340.0;
/// 框与框之间的空隙（也是拖动改框高的把手）、框最矮多高
const SPLIT: f32 = 16.0;
pub const MIN_BOX: f32 = 40.0;
/// 分隔线在空隙里的位置：每段内容最后一行字下面带着 2 的行距，下一段的标题紧贴段顶，线比正中往上 1，上下留白才一样
const RULE: f32 = SPLIT / 2.0 - 1.0;
/// 账号卡之间的间距
const CARD_GAP: f32 = 8.0;
/// 会话、技能的行高（两行字）
const ROW2: f32 = 2.0 * ROW;
/// 框的个数：额度、会话、Token、技能
pub const BOXES: usize = 4;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Session(u32),
    Day(i32),
    /// 按周显示时的一周，值是周一的天号
    Week(i32),
    Toggle(bool),
    /// Token 合计算不算缓存读取的开关
    Cache,
    /// 账号列表里第 i 个其他账号，点了切过去
    Account(usize),
    Quota,
    /// VPS Token 的账号卡，点了和 Quota 一样刷新
    Remote,
    Waker,
    Heatmap,
    Projects,
    Prediction(usize),
    /// 右上角的置顶按钮
    Pin,
    /// 第 i 个框下方的拖动条
    Split(usize),
    /// 技能列表的第 i 行，点了复制 /名称
    Skill(usize),
}

pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub struct Fonts {
    /// 13 号正文、粗体；12 号状态栏
    pub body: IDWriteTextFormat,
    pub bold: IDWriteTextFormat,
    pub status: IDWriteTextFormat,
    /// PROSE (LXGW WenKai Mono): session titles and skill summaries
    pub prose: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
    /// 正文大小、可换行、顶对齐，提示卡片的正文用
    pub wrap: IDWriteTextFormat,
    /// 一个等宽字符的宽度（正文）
    pub cw: f32,
}

impl Fonts {
    pub fn new(dw: &IDWriteFactory) -> Fonts {
        let bundled = fontload::Bundled::load(dw);
        // A family whose file is missing falls back to Cascadia Mono; CJK glyphs a face lacks go to YaHei UI
        let mk_family = |family: &str, size: f32, weight: DWRITE_FONT_WEIGHT| unsafe {
            let (name, coll) = if bundled.has(family) { (family, bundled.collection.as_ref()) } else { (theme::FONT_FALLBACK, None) };
            let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let f = dw.CreateTextFormat(PCWSTR(name.as_ptr()), coll, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("zh-CN")).expect("创建字体失败");
            if let Some(fb) = &bundled.fallback {
                if let Ok(f1) = f.cast::<IDWriteTextFormat1>() {
                    let _ = f1.SetFontFallback(fb);
                }
            }
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            if let Ok(sign) = dw.CreateEllipsisTrimmingSign(&f) {
                let _ = f.SetTrimming(&trim, &sign);
            }
            f
        };
        let mk = |size: f32, weight: DWRITE_FONT_WEIGHT| mk_family(theme::FONT, size, weight);
        // Win11 的图标字体是 Segoe Fluent Icons，Win10 只有 Segoe MDL2 Assets，两者用到的码位相同（只有提示卡片的状态方块用）
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
        let wrap = mk(theme::SIZE_BODY, DWRITE_FONT_WEIGHT_NORMAL);
        unsafe {
            let _ = wrap.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = wrap.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        let body = mk(theme::SIZE_BODY, DWRITE_FONT_WEIGHT_NORMAL);
        let cw = unsafe {
            let m: Vec<u16> = "MMMMMMMMMM".encode_utf16().collect();
            dw.CreateTextLayout(&m, &body, 1000.0, 100.0).ok().and_then(|l| {
                let mut tm = DWRITE_TEXT_METRICS::default();
                l.GetMetrics(&mut tm).ok().map(|_| tm.widthIncludingTrailingWhitespace / 10.0)
            })
        }
        .unwrap_or(7.8);
        let prose = mk_family(theme::FONT_PROSE, theme::SIZE_BODY, DWRITE_FONT_WEIGHT_NORMAL);
        Fonts { body, bold: mk(theme::SIZE_BODY, DWRITE_FONT_WEIGHT_BOLD), status: mk(theme::SIZE_STATUS, DWRITE_FONT_WEIGHT_NORMAL), prose, icon, wrap, cw }
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

    /// 在 r 里画一行字，返回文字实际宽度
    pub fn text(&self, s: &str, r: D2D_RECT_F, f: &IDWriteTextFormat, c: D2D1_COLOR_F, al: Align) -> f32 {
        self.draw_text(s, r, f, c, al, false, false)
    }

    /// 和 text 一样，但数字等宽：位数相同的时间各行一样宽，列才对得齐
    pub fn num(&self, s: &str, r: D2D_RECT_F, f: &IDWriteTextFormat, c: D2D1_COLOR_F, al: Align) -> f32 {
        self.draw_text(s, r, f, c, al, false, true)
    }

    /// 用 num 右对齐画这行字时，笔画左边离 r 的右边多远（不画）
    pub fn width(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        let Some(l) = self.layout(s, f, 1000.0, 100.0, true) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        // 笔画伸出排版框为正、缩进为负
        let o = unsafe {
            let _ = l.GetMetrics(&mut m);
            l.GetOverhangMetrics().unwrap_or_default()
        };
        m.width + o.left
    }

    /// 排一行字；num 时数字等宽（Segoe UI Variable 的数字默认不等宽，“1” 比别的窄）
    fn layout(&self, s: &str, f: &IDWriteTextFormat, w: f32, h: f32, num: bool) -> Option<IDWriteTextLayout> {
        let s: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let l = self.dw.CreateTextLayout(&s, f, w.max(1.0), h.max(1.0)).ok()?;
            if num {
                let t = self.dw.CreateTypography().ok()?;
                let _ = t.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 });
                let _ = l.SetTypography(&t, DWRITE_TEXT_RANGE { startPosition: 0, length: s.len() as u32 });
            }
            Some(l)
        }
    }

    fn draw_text(&self, s: &str, r: D2D_RECT_F, f: &IDWriteTextFormat, c: D2D1_COLOR_F, al: Align, ink: bool, num: bool) -> f32 {
        let Some(l) = self.layout(s, f, r.right - r.left, r.bottom - r.top, num) else { return 0.0 };
        unsafe {
            let _ = l.SetTextAlignment(match al {
                Align::L => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::C => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::R => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            // 笔画伸出（正）或缩进（负）排版框左右边多少
            let o = if ink { l.GetOverhangMetrics().unwrap_or_default() } else { DWRITE_OVERHANG_METRICS::default() };
            let x = r.left + match al {
                Align::L => o.left,
                Align::C => (o.left - o.right) / 2.0,
                Align::R => -o.right,
            };
            if self.shadow {
                self.color(rgba(0.0, 0.0, 0.0, 0.55 * c.a));
                self.dc.DrawTextLayout(Vector2 { X: x + 1.0, Y: r.top + 1.0 }, &l, self.br, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            self.color(c);
            self.dc.DrawTextLayout(Vector2 { X: x, Y: r.top }, &l, self.br, D2D1_DRAW_TEXT_OPTIONS_NONE);
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
    /// 各框内容的自然高度，下一帧据此定框高
    pub nat: [f32; BOXES],
    /// 各框实际的 (顶, 高)
    pub boxes: [(f32, f32); BOXES],
}

/// 框高：用户拖过的高度（不超过内容），没拖过就是内容高度；放不下屏幕（max_h）时先压会话框，再压 Token 框，最后压技能框
pub fn box_heights(user: [f32; BOXES], nat: [f32; BOXES], fixed: f32, max_h: f32) -> [f32; BOXES] {
    let mut b = [0.0; BOXES];
    for i in 0..BOXES {
        b[i] = if user[i] > 0.0 { user[i].min(nat[i]) } else { nat[i] };
        b[i] = b[i].max(MIN_BOX.min(nat[i]));
    }
    let mut over = fixed + b.iter().sum::<f32>() - max_h;
    for i in [1, 2, 3, 0] {
        if over > 0.0 {
            let cut = over.min((b[i] - MIN_BOX.min(nat[i])).max(0.0));
            b[i] -= cut;
            over -= cut;
        }
    }
    b
}

/// 1 px 横线
fn rule(p: &P, th: &Theme, x0: f32, x1: f32, y: f32) {
    p.fill(rect(x0, y, x1, y + 1.0), 0.0, th.border);
}

/// 章节标题：`$` 提示符加粗体标题，后面是 `# 注释`（灰）。返回标题和注释占到的右边
fn heading(p: &P, a: &App, x: f32, y: f32, r: f32, title: &str, note: &str) {
    let (th, f) = (&a.theme, &a.fonts);
    p.text("$", rect(x, y, x + f.cw * 2.0, y + ROW), &f.body, th.accent, Align::L);
    let tw = p.text(title, rect(x + f.cw * 2.0, y, r, y + ROW), &f.bold, th.text, Align::L);
    if !note.is_empty() {
        p.text(&format!("# {note}"), rect(x + f.cw * 2.0 + tw + f.cw * 1.5, y, r, y + ROW), &f.body, th.muted, Align::L);
    }
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

    let mut y = 6.0;
    let head = rect(l, y, r, y + ROW + 4.0);
    p.text("$", rect(l, y, l + f.cw * 2.0, y + ROW + 4.0), &f.body, th.accent, Align::L);
    p.text("claude", rect(l + f.cw * 2.0, y, r, y + ROW + 4.0), &f.bold, th.text, Align::L);
    // 置顶：方括号标签，开着是强调色
    let pin_text = tr("[置顶]", "[pin]");
    let pin_w = p.width(pin_text, &f.body);
    let pin = rect(r - pin_w - 4.0, y + 2.0, r + 4.0, y + ROW + 2.0);
    if a.hover == Some(Hit::Pin) {
        p.fill(pin, theme::RADIUS, th.raised);
    }
    p.text(pin_text, head, &f.body, if a.cfg.topmost { th.accent } else { th.muted }, Align::R);
    hits.push((pin, Hit::Pin));
    let zone = match a.cfg.tz { 1 => tr("本机时间", "Local time"), 2 => "UTC", _ => tr("北京时间", "Beijing time") };
    p.text(&format!("# {zone}"), rect(l + f.cw * 10.0, y, pin.left - 8.0, y + ROW + 4.0), &f.body, th.muted, Align::R);
    y += ROW + 8.0;
    y = service_rows(&p, a, w, y, &mut hits);
    rule(&p, th, l, r, y + RULE);
    y += SPLIT;

    // 四个框：额度、会话、Token、技能。各自裁剪、可滚动，框下方的空隙可以拖动改框高
    let token_on = a.cfg.heatmap || a.cfg.projects;
    let skills_on = !a.skills.is_empty();
    let shown = |i: usize| match i {
        2 => token_on,
        3 => skills_on,
        _ => true,
    };
    let count = (0..BOXES).filter(|&i| shown(i)).count() as f32;
    let fixed = y + count * SPLIT + 2.0 * ROW + 4.0 + STATUS_H + 1.0;
    let mut natural = a.nat;
    for i in 0..BOXES {
        if !shown(i) {
            natural[i] = 0.0;
        }
    }
    let bh = box_heights(a.cfg.heights, natural, fixed, max_h);
    let mut nat = [0.0; BOXES];
    let mut boxes = [(0.0, 0.0); BOXES];
    for i in 0..BOXES {
        if !shown(i) {
            continue;
        }
        let (top, hgt) = (y, bh[i]);
        let scroll = a.scroll[i].min((a.nat[i] - hgt).max(0.0)).max(0.0);
        // 会话、技能框的标题固定在框顶，下面的列表滚动
        let head = match i {
            1 => session_head(&p, a, w, top),
            3 => skills_head(&p, a, w, top),
            _ => 0.0,
        };
        let (ctop, vis) = (top + head, (hgt - head).max(0.0));
        let mut local = Vec::new();
        unsafe {
            p.dc.PushAxisAlignedClip(&rect(0.0, ctop, w, top + hgt), D2D1_ANTIALIAS_MODE_ALIASED);
            p.dc.SetTransform(&Matrix3x2::translation(0.0, ctop - scroll));
            nat[i] = head
                + match i {
                    0 => account_rows(&p, a, w, &mut local),
                    1 => session_box(&p, a, w, now, &mut local),
                    2 => token_box(&p, a, w, &mut local),
                    _ => skill_box(&p, a, w, &mut local),
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
            p.fill(rect(w - 5.0, ty, w - 3.0, ty + thumb), 0.0, th.muted);
        }
        boxes[i] = (top, hgt);
        y = top + hgt;
        // 框下方的拖动条：悬停或拖动时显出一道短横线
        let gap = SPLIT;
        rule(&p, th, l, r, y + RULE);
        let zone = rect(0.0, y, w, y + gap);
        if a.hover == Some(Hit::Split(i)) || a.dragging == Some(i) {
            let c = w / 2.0;
            p.fill(rect(c - 18.0, y + RULE - 1.0, c + 18.0, y + RULE + 2.0), 0.0, th.muted);
        }
        hits.push((zone, Hit::Split(i)));
        y += gap;
    }
    for (i, name) in ["Codex", "Claude"].iter().enumerate() {
        let label = match a.predictions.as_ref().map(|feeds| &feeds[i]) {
            Some(Ok(prediction)) if now - prediction.polled <= 3 * 3600 => prediction.label(),
            Some(Ok(_)) => tr("信息较旧", "Stale"),
            Some(Err(_)) => tr("未知", "Unknown"),
            None => tr("查询中", "Loading"),
        };
        let row = rect(l, y, r, y + ROW);
        if a.hover == Some(Hit::Prediction(i)) {
            p.fill(rect(1.0, y, w - 1.0, y + ROW), 0.0, th.raised);
        }
        p.text(name, row, &f.body, th.muted, Align::L);
        p.text(label, row, &f.body, th.text, Align::R);
        hits.push((row, Hit::Prediction(i)));
        y += ROW;
    }
    y += 4.0;
    status_bar(&p, a, w, y, now);
    y += STATUS_H + 1.0;
    // 窗口外框：1 px border
    p.color(th.border);
    unsafe { a.gfx.dc.DrawRectangle(&rect(0.5, 0.5, w - 0.5, y - 0.5), &a.gfx.brush, 1.0, None) };
    Frame { h: y, hits, nat, boxes }
}

/// 底部状态栏（surface 底）：退出码标签、会话数、5 小时额度，右边是时钟
fn status_bar(p: &P, a: &App, w: f32, y: f32, now: i64) {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    p.fill(rect(1.0, y, w - 1.0, y + STATUS_H + 1.0), 0.0, th.surface);
    rule(p, th, 1.0, w - 1.0, y);
    let row = rect(l, y + 1.0, r, y + 1.0 + STATUS_H);
    let (tag, color) = match &a.quota {
        Fetch::Ok(_) => ("[0] OK", th.green),
        Fetch::NoLogin => ("[-] --", th.muted),
        _ => ("[1] ERR", th.red),
    };
    let mut x = l + p.text(tag, row, &f.status, color, Align::L) + 8.0;
    let busy = a.sessions.iter().filter(|s| s.state != State::Idle).count();
    let sess = if en() { format!("{busy}/{} sessions", a.sessions.len()) } else { format!("{busy}/{} 会话", a.sessions.len()) };
    let quota_5h = match &a.quota {
        Fetch::Ok(ws) => ws.iter().find(|wd| wd.kind == Kind::Session).map(|wd| format!("5h {:.0}%", wd.pct.clamp(0.0, 100.0))),
        _ => None,
    };
    for part in std::iter::once(sess).chain(quota_5h) {
        x += p.text("│", rect(x, row.top, r, row.bottom), &f.status, th.border, Align::L) + 8.0;
        x += p.text(&part, rect(x, row.top, r, row.bottom), &f.status, th.text, Align::L) + 8.0;
    }
    let zone = match a.cfg.tz { 1 => tr("本机", "Local"), 2 => "UTC", _ => tr("北京", "CST") };
    let clock = time::stamp(now, a.cfg.tz);
    p.num(&format!("{zone} {}", &clock[clock.len() - 5..]), rect(x, row.top, r, row.bottom), &f.status, th.muted, Align::R);
}

/// Compact service status; the wake-up row opens the existing token console.
fn service_rows(p: &P, a: &App, w: f32, mut y: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f, l, r) = (&a.theme, &a.fonts, PAD, w - PAD);
    let key_w = f.cw * 9.0;
    let sync = a.sync.as_ref().map(|s| {
        let state = if s.ok { tr("已同步", "Synced") } else { tr("失败", "Failed") };
        format!("{state} · {}", time::stamp(s.at, a.cfg.tz))
    }).unwrap_or_else(|| tr("未配置", "Not configured").into());
    p.text(tr("同步", "Sync"), rect(l, y, l + key_w, y + ROW), &f.body, th.muted, Align::L);
    p.text(&sync, rect(l + key_w, y, r, y + ROW), &f.body, if a.sync.as_ref().is_some_and(|s| !s.ok) { th.orange } else { th.text }, Align::R);
    y += ROW;
    let wake = match &a.waker {
        Some(Ok(s)) => s.summary(a.cfg.tz),
        Some(Err(_)) => tr("VPS 暂不可达", "VPS unreachable").into(),
        None if crate::waker::linked(&a.data) => tr("查询中", "Loading").into(),
        None => if a.cfg.ping { tr("本机自动打卡", "Local idle wake-up") } else { tr("未启用", "Off") }.into(),
    };
    let row = rect(l, y, r, y + ROW);
    if crate::waker::linked(&a.data) {
        if a.hover == Some(Hit::Waker) {
            p.fill(rect(1.0, y, w - 1.0, y + ROW), 0.0, th.raised);
        }
        hits.push((row, Hit::Waker));
    }
    p.text(tr("唤醒", "Wake-up"), rect(l, y, l + key_w, y + ROW), &f.body, th.muted, Align::L);
    p.text(&wake, rect(l + key_w, y, r, y + ROW), &f.body, th.text, Align::R);
    y += ROW;
    if let Some(Ok(status)) = &a.waker {
        for line in status.details(a.cfg.tz) {
            p.text(&line, rect(l + key_w, y, r, y + ROW), &f.body, th.muted, Align::R);
            y += ROW;
        }
    }
    y
}

fn account_rows(p: &P, a: &App, w: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f, l, r) = (&a.theme, &a.fonts, PAD, w - PAD);
    heading(p, a, l, 0.0, r, tr("账号", "accounts"), "");
    let mut y = ROW + 4.0;
    let local = !a.email.is_empty() || !matches!(a.quota, Fetch::NoLogin);
    if local || (!crate::waker::linked(&a.data) && a.accounts.is_empty()) {
        let note = if a.stale { tr("刷新失败，显示上次数据", "Refresh failed; showing previous data") } else { "" };
        y = account_card(p, a, w, y, &a.email, &a.plan, tr("本机", "Local"), &a.quota, note, Hit::Quota, hits);
    }
    for (i, account) in a.accounts.iter().enumerate() {
        let plan = quota::creds(&account.dir).map(|c| c.plan).unwrap_or_default();
        y = account_card(p, a, w, y, &account.email, &plan, tr("点击切换", "Click to switch"), &account.quota, "", Hit::Account(i), hits);
    }
    let remotes = a.identity.as_ref().and_then(|r| r.as_ref().ok()).filter(|_| crate::waker::linked(&a.data));
    let mut shown: Vec<&str> = a.accounts.iter().map(|a| a.email.as_str()).chain([a.email.as_str()]).collect();
    for remote in remotes.into_iter().flatten().filter(|r| r.has_usage()) {
        // 本机已登录的账号、同一账号的另一个 Token 不再重复显示
        if !remote.email.is_empty() && shown.contains(&remote.email.as_str()) { continue }
        shown.push(&remote.email);
        let q = Fetch::Ok(remote.quota.clone().unwrap_or_default());
        let note = if remote.result == "ok" { "" } else { remote.note() };
        y = account_card(p, a, w, y, &remote.email, &remote.plan, &format!("VPS …{}", remote.tail), &q, note, Hit::Remote, hits);
    }
    let _ = (th, f);
    (y - CARD_GAP).max(ROW)
}

/// 一张账号卡：邮箱一行（右边是等级和来源），5 小时和周额度各一行（块状进度条夹在标签和百分比之间）。
/// 悬停时 raised 底色，当前账号前面是 ▸
fn account_card(p: &P, a: &App, w: f32, mut y: f32, email: &str, plan: &str, source: &str, quota: &Fetch, note: &str, hit: Hit, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f, l, r) = (&a.theme, &a.fonts, PAD, w - PAD);
    let indent = f.cw * 2.0;
    let current = !email.is_empty() && email == a.email;
    let status = if !note.is_empty() { note } else {
        match quota {
            Fetch::Expired => tr("登录已过期", "Login expired"),
            Fetch::NoLogin => tr("尚未获取额度", "Usage unavailable"),
            Fetch::Changed(_) => tr("无法查询额度：权限或接口限制", "Usage unavailable: access or API restriction"),
            Fetch::Err(_) => tr("额度查询中或暂不可用", "Usage loading or unavailable"),
            _ => "",
        }
    };
    let card = rect(1.0, y, w - 1.0, y + 3.0 * ROW + if status.is_empty() { 0.0 } else { ROW });
    if a.hover == Some(hit) {
        p.fill(card, theme::RADIUS, th.raised);
    }
    hits.push((card, hit));
    if current {
        p.text("▸", rect(l, y, l + indent, y + ROW), &f.body, th.accent, Align::L);
    }
    let tag = p.text(if current { tr("当前账号", "Current") } else { source }, rect(l, y, r, y + ROW), &f.body, if current { th.accent } else { th.muted }, Align::R);
    let plan_w = if plan.is_empty() { 0.0 } else { p.text(plan, rect(l, y, r - tag - 8.0, y + ROW), &f.body, th.muted, Align::R) + 8.0 };
    p.text(if email.is_empty() { tr("邮箱未获取", "Email unavailable") } else { email }, rect(l + indent, y, r - tag - plan_w - 8.0, y + ROW), &f.bold, th.text, Align::L);
    y += ROW;
    // 列固定，各行、各张卡的进度条才对齐：重置时间那列按 “MM-DD HH:MM” 量，百分比列四个字符宽，右对齐
    let reset_w = p.width(&format!("{} 00-00 00:00", tr("重置", "Resets")), &f.body);
    let pct_r = r - reset_w - 14.0;
    let pct_l = pct_r - f.cw * 4.0;
    let (bar_l, bar_r) = (l + indent + f.cw * 7.0, pct_l - 8.0);
    for (kind, label) in [(Kind::Session, tr("5 小时", "5-hour")), (Kind::Week, tr("周额度", "Weekly"))] {
        let window = match quota { Fetch::Ok(ws) => ws.iter().find(|wd| wd.kind == kind), _ => None };
        let percent = window.map(|wd| format!("{:.0}%", wd.pct.clamp(0.0, 100.0))).unwrap_or_else(|| "—".into());
        let reset = window.and_then(|wd| wd.resets).map(|t| time::stamp(t, a.cfg.tz)).unwrap_or_else(|| "—".into());
        p.text(label, rect(l + indent, y, bar_l, y + ROW), &f.body, th.muted, Align::L);
        p.fill(rect(bar_l, y + 6.0, bar_r, y + 14.0), 0.0, th.raised);
        if let Some(wd) = window.filter(|wd| wd.pct > 0.0) {
            p.fill(rect(bar_l, y + 6.0, bar_l + (bar_r - bar_l) * wd.pct.clamp(0.0, 100.0) as f32 / 100.0, y + 14.0), 0.0, th.level(wd.pct));
        }
        p.num(&percent, rect(pct_l, y, pct_r, y + ROW), &f.body, window.map(|wd| th.level(wd.pct)).unwrap_or(th.muted), Align::R);
        p.num(&format!("{} {reset}", tr("重置", "Resets")), rect(r - reset_w - 2.0, y, r, y + ROW), &f.body, th.muted, Align::R);
        y += ROW;
    }
    if !status.is_empty() {
        p.text(status, rect(l + indent, y, r, y + ROW), &f.body, th.muted, Align::L);
        y += ROW;
    }
    y + CARD_GAP
}

/// 会话框的标题行，画在框顶不滚动，返回高度
fn session_head(p: &P, a: &App, w: f32, y: f32) -> f32 {
    let busy = a.sessions.iter().filter(|s| s.state != State::Idle).count();
    let note = if a.sessions.is_empty() {
        String::new()
    } else if en() {
        format!("{busy} active / {}", a.sessions.len())
    } else {
        format!("{busy} 个进行中 / 共 {}", a.sessions.len())
    };
    heading(p, a, PAD, y, w - PAD, tr("会话", "sessions"), &note);
    ROW + 4.0
}

/// 技能框的标题行，同样固定在框顶
fn skills_head(p: &P, a: &App, w: f32, y: f32) -> f32 {
    let note = if en() { format!("{} · click to copy", a.skills.len()) } else { format!("{} 个 · 点击复制", a.skills.len()) };
    heading(p, a, PAD, y, w - PAD, tr("技能", "skills"), &note);
    ROW + 4.0
}

fn session_box(p: &P, a: &App, w: f32, now: i64, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let indent = f.cw * 2.0;
    let mut y = 0.0;
    if a.sessions.is_empty() {
        p.text(tr("没有运行中的会话", "No running sessions"), rect(l, y, r, y + ROW), &f.body, th.muted, Align::L);
        y += ROW;
    }
    for s in &a.sessions {
        let row = rect(1.0, y, w - 1.0, y + ROW2);
        if a.hover == Some(Hit::Session(s.pid)) {
            p.fill(row, theme::RADIUS, th.raised);
        }
        hits.push((row, Hit::Session(s.pid)));
        let info = a.info.get(&s.id);
        let title = a.desk_titles.get(&s.id).cloned().or_else(|| info.and_then(|i| i.title.clone())).unwrap_or_else(|| s.name.clone());
        let (color, state) = match &s.state {
            State::Waiting(wf) => (th.orange, waiting_text(wf).to_string()),
            State::Busy => (th.green, if en() { format!("Running · {}", fmt_dur(now - s.since)) } else { format!("运行中 · {}", fmt_dur(now - s.since)) }),
            State::Idle => (th.muted, if en() { format!("Idle · {}", fmt_dur(now - s.since)) } else { format!("空闲 · {}", fmt_dur(now - s.since)) }),
        };
        let idle = s.state == State::Idle;
        // State square (drawn, not a glyph: Sarasa sets ● ○ two cells wide): filled while active, outline when idle
        let dot = rect(l + 1.0, y + (ROW - 7.0) / 2.0, l + 8.0, y + (ROW + 7.0) / 2.0);
        if idle {
            p.color(color);
            unsafe { p.dc.DrawRectangle(&rect(dot.left + 0.5, dot.top + 0.5, dot.right - 0.5, dot.bottom - 0.5), p.br, 1.0, None) };
        } else {
            p.fill(dot, 0.0, color);
        }
        let sw = p.text(&state, rect(l + 120.0, y, r, y + ROW), &f.body, if idle { th.muted } else { color }, Align::R);
        p.text(&title, rect(l + indent, y, r - sw - 10.0, y + ROW), &f.prose, if idle { th.muted } else { th.text }, Align::L);
        let mut sub = String::new();
        if s.desktop {
            sub += tr("桌面版 · ", "Desktop · ");
        }
        sub += &s.project;
        if let (false, Some(t)) = (idle, info.and_then(|i| i.tool.as_ref())) {
            sub += " · ";
            sub += t;
        }
        p.text(&sub, rect(l + indent, y + ROW, r, y + ROW2), &f.body, th.muted, Align::L);
        y += ROW2;
    }
    y
}

/// 技能列表：`/名称`（强调色）一行，一句话摘要（灰，超出省略）一行；点击复制 `/名称`
fn skill_box(p: &P, a: &App, w: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let mut y = 0.0;
    for (i, s) in a.skills.iter().enumerate() {
        let row = rect(1.0, y, w - 1.0, y + ROW2);
        if a.hover == Some(Hit::Skill(i)) {
            p.fill(row, theme::RADIUS, th.raised);
        }
        hits.push((row, Hit::Skill(i)));
        let copied = a.copied.as_deref() == Some(s.name.as_str());
        let tag = if copied { p.text(tr("[0] 已复制", "[0] copied"), rect(l, y, r, y + ROW), &f.body, th.green, Align::R) + 8.0 } else { 0.0 };
        p.text(&format!("/{}", s.name), rect(l, y, r - tag, y + ROW), &f.body, th.accent, Align::L);
        p.text(&s.summary, rect(l, y + ROW, r, y + ROW2), &f.prose, th.muted, Align::L);
        y += ROW2;
    }
    y
}

fn token_box(p: &P, a: &App, w: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let y = if a.cfg.heatmap { heatmap_box(p, a, w, hits) } else { 0.0 };
    if a.cfg.projects { project_rows(p, a, w, if y > 0.0 { y + 12.0 } else { 0.0 }, hits) } else { y }
}

/// 标题行右上角的 `[×]`：关掉这一块（和菜单里的开关一样）
fn close_button(p: &P, a: &App, r: f32, y: f32, hit: Hit, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let cw = p.width("[×]", &f.body);
    let b = rect(r - cw - 4.0, y, r + 2.0, y + ROW);
    if a.hover == Some(hit) {
        p.fill(b, theme::RADIUS, th.raised);
    }
    p.text("[×]", rect(b.left, y, r, y + ROW), &f.body, th.muted, Align::R);
    hits.push((b, hit));
    b.left
}

fn heatmap_box(p: &P, a: &App, w: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f) = (&a.theme, &a.fonts);
    let (l, r) = (PAD, w - PAD);
    let mut y = 0.0;
    let today = time::today();
    let weeks = (((r - l - DAY_LABEL_W) + (STEP - CELL)) / STEP).floor().max(4.0) as i32;
    let gx = l + DAY_LABEL_W;
    // 每列的左边：周数取整后除不尽的余量分进列间隙，最后一列正好收在右边；取整到像素，格子边缘不发虚
    let col = |c: i32| (gx + c as f32 * (r - gx - CELL) / (weeks - 1) as f32).round();
    let start = monday(today) - (weeks - 1) * 7;
    let cr = a.cfg.cache_read;
    // 标题行右边从右往左：[×]、日|周、[x] 缓存
    let mut x = close_button(p, a, r, y, Hit::Heatmap, hits) - 10.0;
    for (label, weekly) in [(tr("周", "Week"), true), (tr("日", "Day"), false)] {
        let bw = p.width(label, &f.body) + 12.0;
        let b = rect(x - bw, y + 1.0, x, y + ROW - 1.0);
        let on = a.cfg.weekly == weekly;
        if on {
            p.fill(b, theme::RADIUS, th.inverse_bg);
        } else if a.hover == Some(Hit::Toggle(weekly)) {
            p.fill(b, theme::RADIUS, th.raised);
        }
        p.text(label, b, if on { &f.bold } else { &f.body }, if on { th.inverse_fg } else { th.muted }, Align::C);
        hits.push((b, Hit::Toggle(weekly)));
        x = b.left;
    }
    x -= 12.0;
    let cache = format!("{} {}", if cr { "[x]" } else { "[ ]" }, tr("缓存", "Cache"));
    let cwid = p.width(&cache, &f.body);
    let sb = rect(x - cwid - 4.0, y, x + 2.0, y + ROW);
    if a.hover == Some(Hit::Cache) {
        p.fill(sb, theme::RADIUS, th.raised);
    }
    p.text(&cache, rect(sb.left, y, x, y + ROW), &f.body, if cr { th.text } else { th.muted }, Align::R);
    hits.push((sb, Hit::Cache));
    let sum: u64 = (start..=today).map(|d| usage::day_total(&a.days, d, cr)).sum();
    let span = if en() { format!("{weeks}w · {}", fmt_tokens(sum)) } else { format!("近 {weeks} 周 · {}", fmt_tokens(sum)) };
    heading(p, a, l, y, sb.left - 4.0, "token", &span);
    y += ROW + 6.0;

    // 月份标签：某一列的周一换了月份就标一下，太挤就跳过
    let mut last_x = -100.0;
    let mut last_m = 0;
    for c in 0..weeks {
        let (_, m, _) = time::civil(start + c * 7);
        let x = col(c);
        if m != last_m && x - last_x >= 36.0 {
            let ml = if en() { ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][m as usize - 1].to_string() } else { format!("{m}月") };
            p.text(&ml, rect(x, y, x + 44.0, y + 16.0), &f.body, th.muted, Align::L);
            last_x = x;
        }
        last_m = m;
    }
    y += 18.0;
    let gy = y;
    let sel = a.sel;
    if !a.cfg.weekly {
        for (i, wd) in [(0, tr("一", "Mon")), (2, tr("三", "Wed")), (4, tr("五", "Fri"))] {
            p.text(wd, rect(l, gy + i as f32 * STEP - 3.0, gx, gy + i as f32 * STEP + CELL + 3.0), &f.body, th.muted, Align::L);
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
                let cell = rect(col(c), gy + rr as f32 * STEP, col(c) + CELL, gy + rr as f32 * STEP + CELL);
                let lv = level(v, &thr);
                p.fill(cell, 0.0, if lv == 0 { th.raised } else { with_a(th.accent, [0.0, 0.32, 0.55, 0.78, 1.0][lv]) });
                if sel == Some(d) || a.hover == Some(Hit::Day(d)) || (sel.is_none() && d == today) {
                    p.color(th.accent);
                    unsafe { a.gfx.dc.DrawRectangle(&rect(cell.left - 1.5, cell.top - 1.5, cell.right + 1.5, cell.bottom + 1.5), &a.gfx.brush, 1.0, None) };
                }
                hits.push((rect(cell.left - 1.5, cell.top - 1.5, cell.right + 1.5, cell.bottom + 1.5), Hit::Day(d)));
            }
        }
    } else {
        let vals: Vec<u64> = (0..weeks).map(|c| (0..7).map(|k| usage::day_total(&a.days, start + c * 7 + k, cr)).sum()).collect();
        let max = vals.iter().copied().max().unwrap_or(0).max(1);
        let hgt = 7.0 * STEP - (STEP - CELL);
        for (c, &v) in vals.iter().enumerate() {
            let x = col(c as i32);
            let wk = start + c as i32 * 7;
            let bh = if v == 0 { 2.0 } else { (hgt * v as f32 / max as f32).max(3.0) };
            let on = sel == Some(wk) || a.hover == Some(Hit::Week(wk)) || (sel.is_none() && c as i32 == weeks - 1);
            p.fill(rect(x, gy + hgt - bh, x + CELL, gy + hgt), 0.0, if v == 0 { th.raised } else if on { th.accent } else { with_a(th.accent, 0.55) });
            hits.push((rect(x - 1.5, gy, x + CELL + 1.5, gy + hgt), Hit::Week(wk)));
        }
        p.text(&fmt_tokens(max), rect(l, gy - 3.0, gx, gy + 13.0), &f.body, th.muted, Align::L);
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
    let legend_w = if a.cfg.weekly { 0.0 } else { 150.0 };
    p.text(&ftxt, rect(l, y, r - legend_w, y + ROW), &f.body, th.text, Align::L);
    if !a.cfg.weekly {
        // 图例右边贴着内容右边（和热力图右边一样），格子两头离“少”“多”一样远
        let mw = p.text(tr("多", "More"), rect(r - 40.0, y, r, y + ROW), &f.body, th.muted, Align::R);
        let x = r - mw - 4.0 - 4.0 * STEP - CELL;
        p.text(tr("少", "Less"), rect(x - 40.0, y, x - 4.0, y + ROW), &f.body, th.muted, Align::R);
        for lv in 0..5 {
            let cx = x + lv as f32 * STEP;
            p.fill(rect(cx, y + 4.0, cx + CELL, y + 4.0 + CELL), 0.0, if lv == 0 { th.raised } else { with_a(th.accent, [0.0, 0.32, 0.55, 0.78, 1.0][lv]) });
        }
    }

    // 和其他框一样，最后一行字下面带 2 的行距
    y + ROW + 2.0
}

fn project_rows(p: &P, a: &App, w: f32, mut y: f32, hits: &mut Vec<(D2D_RECT_F, Hit)>) -> f32 {
    let (th, f, l, r) = (&a.theme, &a.fonts, PAD, w - PAD);
    let today = time::today();
    let (sum, projects, _) = usage::breakdown(&a.days, today, today, a.cfg.cache_read);
    let left = close_button(p, a, r, y, Hit::Projects, hits);
    heading(p, a, l, y, left - 4.0, tr("今日项目消耗", "today's projects"), &fmt_tokens(sum));
    y += ROW + 4.0;
    if projects.is_empty() {
        p.text(tr("今天暂无消耗", "No usage today"), rect(l, y, r, y + ROW), &f.body, th.muted, Align::L);
        return y + ROW;
    }
    let max = projects.first().map_or(1, |e| e.1).max(1);
    // The name column starts on a character cell, so bars line up with the grid
    let col = l + (((r - l) * 0.47) / f.cw).floor() * f.cw;
    for (name, value) in projects {
        p.text(usage::base_name(&name), rect(l, y, col - 8.0, y + ROW), &f.body, th.text, Align::L);
        let vw = p.text(&fmt_tokens(value), rect(l, y, r, y + ROW), &f.body, th.muted, Align::R);
        let bar = (r - vw - 12.0 - col).max(0.0);
        p.fill(rect(col, y + 6.0, col + bar * value as f32 / max as f32, y + 14.0), 0.0, with_a(th.accent, 0.75));
        y += ROW;
    }
    y + 2.0
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
        let nat = [100.0, 400.0, 300.0, 0.0];
        // 没拖过：内容多高框多高；拖过：不超过内容
        assert_eq!(box_heights([0.0; 4], nat, 50.0, 2000.0), nat);
        assert_eq!(box_heights([0.0, 200.0, 999.0, 0.0], nat, 50.0, 2000.0), [100.0, 200.0, 300.0, 0.0]);
        // 拖得比最矮还矮按最矮算；内容本身比最矮还矮就按内容
        assert_eq!(box_heights([10.0, 10.0, 0.0, 0.0], [30.0, 400.0, 300.0, 0.0], 50.0, 2000.0), [30.0, MIN_BOX, 300.0, 0.0]);
        // 屏幕放不下：先压会话框，压到最矮再压 Token 框
        assert_eq!(box_heights([0.0; 4], nat, 50.0, 650.0), [100.0, 200.0, 300.0, 0.0]);
        assert_eq!(box_heights([0.0; 4], nat, 50.0, 400.0), [100.0, MIN_BOX, 210.0, 0.0]);
        // Many accounts must scroll too, leaving the status footer on screen.
        assert_eq!(box_heights([0.0; 4], [1000.0, 100.0, 200.0, 0.0], 100.0, 600.0), [420.0, MIN_BOX, MIN_BOX, 0.0]);
        assert_eq!(box_heights([0.0; 4], [100.0, 40.0, 0.0, 0.0], 80.0, 600.0), [100.0, 40.0, 0.0, 0.0]);
        // 技能框最后被压
        assert_eq!(box_heights([0.0; 4], [100.0, 200.0, 100.0, 300.0], 50.0, 600.0), [100.0, 50.0, 100.0, 300.0]);
        assert_eq!(box_heights([0.0; 4], [100.0, 200.0, 100.0, 300.0], 50.0, 400.0), [100.0, MIN_BOX, MIN_BOX, 170.0]);
    }

    #[test]
    fn week_start() {
        // 2026-10-18 周日 → 周一是 10-12
        assert_eq!(monday(20744), 20738);
        assert_eq!(monday(20738), 20738);
    }
}
