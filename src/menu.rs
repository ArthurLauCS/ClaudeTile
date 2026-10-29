//! 右键菜单：自己画的面板，替代系统菜单（系统菜单不跟深色主题，也放不下滑块）。
//! 透明度滑块可以直接拖；开关、分段选项点了面板不关，方便连着调；动作项点了就关。点到面板外面、按 Esc 关闭。
//! 面板底色不透明，不受磁贴透明度影响。键盘：上下选项、左右调滑块和分段、回车或空格确认。

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::config::{en, tr};
use crate::gfx::Surface;
use crate::theme::{self, with_a};
use crate::view::{Align, P, Theme, inside, rect};
use crate::{App, app, sys};

pub const MENU_CLASS: PCWSTR = w!("ClaudeTileMenu");
const W: f32 = 272.0;
const ROW: f32 = 28.0;
const EDGE: f32 = 6.0;
/// 内容左右边：文字、开关、分段条、分隔线、滑块（连同圆钮）都在 X 和 W - X 之间
const X: f32 = 14.0;
/// 滑块圆钮的半径：圆钮中心只走到离两边 THUMB 处，拉满也不出内容边
const THUMB: f32 = 4.0;
pub const OPACITY_MIN: u32 = 20;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Id {
    Opacity,
    Top,
    Ping,
    Waker,
    Heatmap,
    Projects,
    Auto,
    Alert(u32),
    Lang(bool),
    Tz(u32),
    AddAccount,
    Refresh,
    Layout,
    Hide,
    Exit,
}

enum Row {
    Slider,
    Switch(Id, &'static str, bool),
    Seg(&'static str, Vec<(Id, &'static str, bool)>),
    Act(Id, &'static str, &'static str),
    Sep,
}

pub struct Menu {
    pub hwnd: HWND,
    surf: Option<Surface>,
    hits: Vec<(D2D_RECT_F, Id)>,
    hover: Option<Id>,
    /// 键盘焦点（hits 里的序号），None 表示没用过键盘
    focus: Option<usize>,
    sliding: bool,
    tracking: bool,
    /// 按下时所在的项，松开时还在同一项上才算点击
    pressed: Option<Id>,
}

fn rows(a: &App) -> Vec<Row> {
    vec![
        Row::Slider,
        Row::Sep,
        Row::Switch(Id::Top, tr("置顶显示", "Always on top"), a.cfg.topmost),
        Row::Switch(Id::Auto, tr("开机自启", "Start with Windows"), sys::autostart()),
        Row::Switch(Id::Heatmap, tr("显示 Token 热力图", "Show token heatmap"), a.cfg.heatmap),
        Row::Switch(Id::Projects, tr("显示今日项目消耗", "Show today's project usage"), a.cfg.projects),
        Row::Act(Id::Layout, "\u{E7A7}", tr("恢复默认布局", "Reset layout")),
        // 语言入口和选项都带英文，切到看不懂的语言后也能找回来
        Row::Seg(tr("语言/Language", "Language"), vec![(Id::Lang(false), "中文/Chinese", !en()), (Id::Lang(true), "English", en())]),
        Row::Seg(tr("时区", "Time zone"), [(0, tr("北京", "Beijing")), (1, tr("本机", "Local")), (2, "UTC")].iter().map(|&(v, s)| (Id::Tz(v), s, a.cfg.tz == v)).collect()),
        Row::Sep,
        // Segoe Fluent Icons：E8FA 添加联系人，E72C 刷新，E7A7 撤销，ED1A 隐藏，E890 显示，E7E8 电源
        Row::Act(Id::AddAccount, "\u{E8FA}", tr("添加账号…", "Add account…")),
        if crate::waker::linked(&a.data) {
            Row::Act(Id::Waker, "\u{E787}", tr("VPS 定时唤醒 / 填写 Token…", "VPS wake-up / Set token…"))
        } else {
            Row::Switch(Id::Ping, tr("空闲时自动打卡（开 5 小时窗口）", "Auto-start idle 5h windows"), a.cfg.ping)
        },
        Row::Seg(tr("额度提醒", "Usage alert"), [70, 80, 90, 0].iter().map(|&v| (Id::Alert(v), alert_label(v), a.cfg.alert == v)).collect()),
        Row::Act(Id::Refresh, "\u{E72C}", tr("立即刷新", "Refresh now")),
        Row::Sep,
        if a.cfg.hidden { Row::Act(Id::Hide, "\u{E890}", tr("显示磁贴", "Show tile")) } else { Row::Act(Id::Hide, "\u{ED1A}", tr("隐藏磁贴", "Hide tile")) },
        Row::Act(Id::Exit, "\u{E7E8}", tr("退出", "Exit")),
    ]
}

fn alert_label(v: u32) -> &'static str {
    match v {
        70 => "70%",
        80 => "80%",
        90 => "90%",
        _ => tr("关闭", "Off"),
    }
}

fn row_h(r: &Row) -> f32 {
    match r {
        Row::Slider => 52.0,
        Row::Seg(..) => 54.0,
        Row::Sep => 9.0,
        _ => ROW,
    }
}

fn height(a: &App) -> f32 {
    2.0 * EDGE + rows(a).iter().map(row_h).sum::<f32>()
}

/// 在屏幕坐标 pt 处弹出；靠近屏幕右边或下边时往左、往上翻（托盘在右下角）
pub fn show(a: &mut App, pt: POINT) {
    close(a);
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let (w, h) = ((W * s) as i32, (height(a) * s) as i32);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let x = if pt.x + w > wa.right { pt.x - w } else { pt.x };
        let y = if pt.y + h > wa.bottom { pt.y - h } else { pt.y };
        let (x, y) = (x.clamp(wa.left, (wa.right - w).max(wa.left)), y.clamp(wa.top, (wa.bottom - h).max(wa.top)));
        // WS_THICKFRAME 只为了要系统阴影，边框由 WM_NCCALCSIZE 去掉
        // Owned popups stay above their owner when its topmost state changes.
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP, MENU_CLASS, w!("ClaudeTile menu"), WS_POPUP | WS_THICKFRAME, x, y, w, h, Some(a.hwnd), None, None, None) else { return };
        let dark = BOOL::from(a.theme.dark);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
        a.menu = Some(Box::new(Menu { hwnd, surf: None, hits: Vec::new(), hover: None, focus: None, sliding: false, tracking: false, pressed: None }));
        let _ = ShowWindow(hwnd, SW_SHOW);
        // 托盘或磁贴刚收到点击，有权把前台给菜单；菜单要在前台才能在点到外面时收到失活消息
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn close(a: &mut App) {
    if let Some(m) = a.menu.take() {
        unsafe {
            let _ = DestroyWindow(m.hwnd);
        }
    }
}

fn draw(a: &App, m: &mut Menu) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(m.hwnd, &mut rc);
        if m.surf.is_none() {
            m.surf = a.gfx.surface(m.hwnd, rc.right as u32, rc.bottom as u32).ok();
        }
        let Some(surf) = m.surf.clone() else { return };
        let dpi = GetDpiForWindow(m.hwnd) as f32;
        if a.gfx.bind(&surf, dpi).is_err() {
            return;
        }
        // 跟磁贴实际用的深浅色走（透明度低于 50% 时磁贴强制深色）
        let th = Theme::panel(a.theme.dark);
        let f = &a.fonts;
        let p = P { dc: &a.gfx.dc, br: &a.gfx.brush, dw: &a.dw, shadow: false };
        let w = rc.right as f32 * 96.0 / dpi;
        let mut hits = Vec::new();
        let focused = m.focus.and_then(|i| m.hits.get(i)).map(|x| x.1);
        let hot = |id: Id| m.hover == Some(id) || focused == Some(id);
        a.gfx.dc.BeginDraw();
        a.gfx.dc.Clear(Some(&th.tint));
        let mut y = EDGE;
        for row in rows(a) {
            let rh = row_h(&row);
            match row {
                Row::Slider => {
                    let v = a.cfg.opacity;
                    p.text(tr("磁贴透明度", "Tile opacity"), rect(X, y + 2.0, w - X, y + 24.0), &f.body, th.text, Align::L);
                    p.text(&format!("{v}%"), rect(X, y + 2.0, w - X, y + 24.0), &f.bold, th.muted, Align::R);
                    let (l, r, cy) = (X + THUMB, w - X - THUMB, y + 36.0);
                    let x = l + (r - l) * (v - OPACITY_MIN) as f32 / (100 - OPACITY_MIN) as f32;
                    p.fill(rect(X, cy - 2.0, w - X, cy + 2.0), theme::RADIUS, th.raised);
                    p.fill(rect(X, cy - 2.0, x, cy + 2.0), theme::RADIUS, th.accent);
                    // 方块滑钮：悬停或拖动时画 1 px 强调色外框
                    p.fill(rect(x - THUMB, cy - 8.0, x + THUMB, cy + 8.0), theme::RADIUS, th.accent);
                    if m.sliding || hot(Id::Opacity) {
                        p.color(th.text);
                        a.gfx.dc.DrawRectangle(&rect(x - THUMB - 0.5, cy - 8.5, x + THUMB + 0.5, cy + 8.5), &a.gfx.brush, 1.0, None);
                    }
                    hits.push((rect(X - 4.0, y + 24.0, w - X + 4.0, y + 50.0), Id::Opacity));
                }
                Row::Switch(id, label, on) => {
                    let r = rect(1.0, y, w - 1.0, y + rh);
                    if hot(id) {
                        p.fill(r, theme::RADIUS, th.raised);
                    }
                    // 终端式复选框：[x] / [ ]，开着是强调色
                    p.text(if on { "[x]" } else { "[ ]" }, rect(X, y, X + 4.0 * f.cw, y + rh), &f.body, if on { th.accent } else { th.muted }, Align::L);
                    p.text(label, rect(X + 4.0 * f.cw, y, w - X, y + rh), &f.body, th.text, Align::L);
                    hits.push((r, id));
                }
                Row::Seg(label, opts) => {
                    p.text(label, rect(X, y + 2.0, w - X, y + 24.0), &f.body, th.muted, Align::L);
                    let bar = rect(X, y + 26.0, w - X, y + 50.0);
                    p.fill(bar, theme::RADIUS, th.raised);
                    let sw = (bar.right - bar.left) / opts.len() as f32;
                    for (k, (id, text, on)) in opts.into_iter().enumerate() {
                        let b = rect(bar.left + k as f32 * sw, bar.top, bar.left + (k + 1) as f32 * sw, bar.bottom);
                        if on {
                            p.fill(b, theme::RADIUS, th.inverse_bg);
                        } else if hot(id) {
                            p.fill(b, theme::RADIUS, th.border);
                        }
                        if focused == Some(id) {
                            p.color(th.accent);
                            a.gfx.dc.DrawRectangle(&rect(b.left + 0.5, b.top + 0.5, b.right - 0.5, b.bottom - 0.5), &a.gfx.brush, 1.0, None);
                        }
                        p.text(text, b, if on { &f.bold } else { &f.body }, if on { th.inverse_fg } else { th.muted }, Align::C);
                        hits.push((b, id));
                    }
                }
                Row::Act(id, _glyph, label) => {
                    let r = rect(1.0, y, w - 1.0, y + rh);
                    let danger = id == Id::Exit && hot(id);
                    if hot(id) {
                        p.fill(r, theme::RADIUS, if danger { with_a(th.red, 0.16) } else { th.raised });
                    }
                    let c = if danger { th.red } else { th.text };
                    // 提示符：命令行式的 $，不用图标
                    p.text("$", rect(X, y, X + 2.0 * f.cw, y + rh), &f.body, if danger { th.red } else { th.accent }, Align::L);
                    p.text(label, rect(X + 2.0 * f.cw, y, w - X, y + rh), &f.body, c, Align::L);
                    hits.push((r, id));
                }
                Row::Sep => p.fill(rect(X, y + 4.0, w - X, y + 5.0), 0.0, th.border),
            }
            y += rh;
        }
        // 1 px border: the popup floats above the desktop, so it carries its own edge
        p.color(th.border);
        a.gfx.dc.DrawRectangle(&rect(0.5, 0.5, w - 0.5, rc.bottom as f32 * 96.0 / dpi - 0.5), &a.gfx.brush, 1.0, None);
        let _ = a.gfx.dc.EndDraw(None, None);
        let _ = a.gfx.present(&surf);
        m.hits = hits;
    }
}

fn hit(m: &Menu, lp: LPARAM) -> Option<Id> {
    let s = unsafe { GetDpiForWindow(m.hwnd) } as f32 / 96.0;
    let (x, y) = ((lp.0 as i16) as f32 / s, ((lp.0 >> 16) as i16) as f32 / s);
    m.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, id)| *id)
}

/// 滑块按鼠标横坐标取值，5% 一档
fn slide(a: &mut App, m: &Menu, lp: LPARAM) {
    let s = unsafe { GetDpiForWindow(m.hwnd) } as f32 / 96.0;
    let x = (lp.0 as i16) as f32 / s;
    let (l, r) = (X + THUMB, W - X - THUMB);
    let t = ((x - l) / (r - l)).clamp(0.0, 1.0);
    let v = OPACITY_MIN + ((t * (100 - OPACITY_MIN) as f32 / 5.0).round() as u32) * 5;
    if v != a.cfg.opacity {
        a.set_opacity(v);
    }
}

pub unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_NCHITTEST => return LRESULT(HTCLIENT as isize),
            WM_ERASEBKGND => return LRESULT(1),
            _ => {}
        }
        let a = app();
        // 菜单先从 App 里拿出来，处理完再放回去：动作可能要改 App 的其他部分，甚至关掉菜单
        let Some(mut m) = a.menu.take_if(|m| m.hwnd == hwnd) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        let mut keep = true;
        let mut redraw = false;
        match msg {
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                draw(a, &mut m);
            }
            WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => keep = false,
            WM_MOUSEMOVE => {
                if !m.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    m.tracking = true;
                }
                if m.sliding {
                    slide(a, &m, lp);
                }
                let h = hit(&m, lp);
                redraw = h != m.hover || m.sliding || m.focus.is_some();
                m.hover = h;
                m.focus = None;
            }
            WM_MOUSELEAVE => {
                m.tracking = false;
                redraw = m.hover.take().is_some();
            }
            WM_LBUTTONDOWN => {
                m.pressed = hit(&m, lp);
                if m.pressed == Some(Id::Opacity) {
                    m.sliding = true;
                    SetCapture(hwnd);
                    slide(a, &m, lp);
                }
                redraw = true;
            }
            WM_LBUTTONUP => {
                if std::mem::take(&mut m.sliding) {
                    let _ = ReleaseCapture();
                    a.save_cfg();
                } else if let Some(id) = hit(&m, lp).filter(|id| m.pressed == Some(*id)) {
                    keep = !a.menu_action(id);
                }
                m.pressed = None;
                redraw = true;
            }
            WM_KEYDOWN => {
                let n = m.hits.len();
                let cur = m.focus.or_else(|| m.hover.and_then(|h| m.hits.iter().position(|x| x.1 == h)));
                match VIRTUAL_KEY(wp.0 as u16) {
                    VK_ESCAPE => keep = false,
                    VK_DOWN | VK_TAB if n > 0 => m.focus = Some(cur.map_or(0, |i| (i + 1) % n)),
                    VK_UP if n > 0 => m.focus = Some(cur.map_or(n - 1, |i| (i + n - 1) % n)),
                    k @ (VK_LEFT | VK_RIGHT) => match cur.map(|i| m.hits[i].1) {
                        Some(Id::Opacity) => {
                            let v = if k == VK_LEFT { a.cfg.opacity.saturating_sub(5).max(OPACITY_MIN) } else { (a.cfg.opacity + 5).min(100) };
                            a.set_opacity(v);
                            a.save_cfg();
                            m.focus = cur;
                        }
                        // 分段选项里左右移动就是换选项
                        Some(Id::Alert(_) | Id::Lang(_) | Id::Tz(_)) => {
                            let i = cur.unwrap_or(0);
                            let j = if k == VK_LEFT { i.saturating_sub(1) } else { (i + 1).min(n - 1) };
                            if std::mem::discriminant(&m.hits[i].1) == std::mem::discriminant(&m.hits[j].1) {
                                keep = !a.menu_action(m.hits[j].1);
                                m.focus = Some(j);
                            }
                        }
                        _ => {}
                    },
                    VK_RETURN | VK_SPACE => {
                        if let Some(i) = cur {
                            keep = !a.menu_action(m.hits[i].1);
                            m.focus = Some(i);
                        }
                    }
                    _ => {}
                }
                redraw = true;
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                m.surf = None;
            }
            _ => {
                a.menu = Some(m);
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
        }
        if keep {
            // 动作可能改了行数（如“隐藏磁贴”变“显示磁贴”）不会改高度；语言切换会改文字，整块重画
            if redraw {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            if a.menu.is_none() {
                a.menu = Some(m);
            }
        } else {
            let _ = DestroyWindow(m.hwnd);
        }
        LRESULT(0)
    }
}
