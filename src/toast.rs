//! 右下角的提示卡片，替代系统托盘气泡（气泡后弹的顶掉先弹的，来源还显示成 exe 名）。
//! 从屏幕右侧滑入，最多叠 3 张，新的在最下面；到时自动滑出，鼠标停在卡片上时不走倒计时。
//! 点卡片执行它的动作（切到那个会话 / 显示磁贴），右上角 × 只关闭。
//! 全屏游戏、演示模式这类不宜打扰的时候先不弹，留在队列里，之后再弹。
//! 动画只挪窗口不重画内容，阴影跟着窗口走。

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::gfx::Surface;
use crate::icon::Tone;
use crate::view::{Align, P, Theme, inside, rect};
use crate::{App, app};

pub const TOAST_CLASS: PCWSTR = w!("ClaudeTileToast");
const W: f32 = 344.0;
const GAP: f32 = 10.0;
const MARGIN: f32 = 14.0;
const MAX: usize = 3;
const T_ANIM: usize = 1;
const T_LIFE: usize = 2;
/// 滑入 / 滑出各多少帧（每帧 16ms）
const FRAMES: u32 = 12;

pub struct Note {
    pub title: String,
    pub text: String,
    pub tone: Tone,
    /// 状态方块里的图标（Segoe Fluent Icons 码位）
    pub glyph: &'static str,
    /// 点卡片时切过去的会话；None 时点卡片显示磁贴
    pub pid: Option<u32>,
}

pub struct Toast {
    hwnd: HWND,
    surf: Option<Surface>,
    note: Note,
    /// 卡片高度（DIP），按正文行数定
    h: f32,
    /// 动画帧：滑入时 0→FRAMES，滑出时 FRAMES→0
    frame: u32,
    leaving: bool,
    hover_close: bool,
    tracking: bool,
}

/// 现在能不能弹：全屏游戏、演示、忙碌时不弹
fn may_show() -> bool {
    unsafe { !matches!(SHQueryUserNotificationState(), Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_APP)) }
}

fn work_area() -> RECT {
    let mut wa = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    wa
}

fn scale() -> f32 {
    unsafe { GetDpiForSystem() as f32 / 96.0 }
}

/// 正文换行后的高度
fn body_h(a: &App, text: &str) -> f32 {
    let w16: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let Ok(l) = a.dw.CreateTextLayout(&w16, &a.fonts.f12w, W - 76.0, 200.0) else { return 18.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&mut m);
        m.height.min(3.0 * 17.0)
    }
}

/// 把排队的提示弹出来，直到叠满 3 张
pub fn pump(a: &mut App) {
    while a.toasts.len() < MAX && !a.notes.is_empty() && may_show() {
        let note = a.notes.pop_front().unwrap();
        let h = (56.0 + body_h(a, &note.text) + 14.0).max(80.0);
        let s = scale();
        unsafe {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                TOAST_CLASS,
                w!("ClaudeTile"),
                WS_POPUP | WS_THICKFRAME,
                0,
                0,
                (W * s) as i32,
                (h * s) as i32,
                None,
                None,
                None,
                None,
            ) else {
                return;
            };
            let dark = BOOL::from(a.theme.dark);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
            let life = if note.pid.is_some() { 10_000 } else { 6_000 };
            a.toasts.push(Toast { hwnd, surf: None, note, h, frame: 0, leaving: false, hover_close: false, tracking: false });
            layout(a);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), T_ANIM, 16, None);
            SetTimer(Some(hwnd), T_LIFE, life, None);
        }
    }
}

/// 按顺序从工作区右下角往上叠；每张的横坐标按动画进度从屏幕外滑到位
fn layout(a: &App) {
    let (wa, s) = (work_area(), scale());
    let mut bottom = wa.bottom - (MARGIN * s) as i32;
    for t in a.toasts.iter().rev() {
        let (w, h) = ((W * s) as i32, (t.h * s) as i32);
        // 缓出：1 - (1 - p)^3
        let p = t.frame as f32 / FRAMES as f32;
        let ease = 1.0 - (1.0 - p).powi(3);
        let home = wa.right - (MARGIN * s) as i32 - w;
        let x = home + ((wa.right - home) as f32 * (1.0 - ease)) as i32;
        unsafe {
            let _ = SetWindowPos(t.hwnd, Some(HWND_TOPMOST), x, bottom - h, w, h, SWP_NOACTIVATE);
        }
        bottom -= h + (GAP * s) as i32;
    }
}

fn draw(a: &App, t: &mut Toast) {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(t.hwnd, &mut rc);
        if t.surf.is_none() {
            t.surf = a.gfx.surface(t.hwnd, rc.right as u32, rc.bottom as u32).ok();
        }
        let Some(surf) = t.surf.clone() else { return };
        let dpi = GetDpiForWindow(t.hwnd) as f32;
        if a.gfx.bind(&surf, dpi).is_err() {
            return;
        }
        // 跟磁贴实际用的深浅色走（透明度低于 50% 时磁贴强制深色）
        let th = Theme::panel(a.theme.dark);
        let f = &a.fonts;
        let p = P { dc: &a.gfx.dc, br: &a.gfx.brush, dw: &a.dw, shadow: false };
        let w = rc.right as f32 * 96.0 / dpi;
        a.gfx.dc.BeginDraw();
        a.gfx.dc.Clear(Some(&th.tint));
        // 左边的状态方块：颜色和托盘图标一致，竖直方向对着右边整段文字的笔画居中：
        // 从应用名的字顶（行框 12 往下 4）到正文末行的字底（正文行框底）
        let (c, ink) = crate::icon::colors(t.note.tone);
        let glyph = t.note.glyph;
        let top = ((16.0 + 50.0 + body_h(a, &t.note.text)) / 2.0 - 18.0).round();
        let ic = rect(16.0, top, 52.0, top + 36.0);
        p.fill(ic, 8.0, c);
        p.text(glyph, ic, &f.icon, ink, Align::C);
        p.text("ClaudeTile", rect(64.0, 12.0, w - 40.0, 28.0), &f.f11, th.muted, Align::L);
        p.text(&t.note.title, rect(64.0, 28.0, w - 16.0, 48.0), &f.f12b, th.text, Align::L);
        let w16: Vec<u16> = t.note.text.encode_utf16().collect();
        if let Ok(l) = a.dw.CreateTextLayout(&w16, &f.f12w, w - 76.0, 3.0 * 17.0) {
            p.color(th.muted);
            a.gfx.dc.DrawTextLayout(windows_numerics::Vector2 { X: 64.0, Y: 50.0 }, &l, &a.gfx.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
        // 右上角关闭：E711
        let x = close_rect(w);
        if t.hover_close {
            p.fill(x, 5.0, th.hover);
        }
        p.text("\u{E711}", x, &f.icon, th.muted, Align::C);
        let _ = a.gfx.dc.EndDraw(None, None);
        let _ = a.gfx.present(&surf);
    }
}

fn close_rect(w: f32) -> D2D_RECT_F {
    rect(w - 36.0, 8.0, w - 8.0, 36.0)
}

fn on_close(hwnd: HWND, lp: LPARAM) -> bool {
    let s = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    inside(&close_rect(rc.right as f32 / s), (lp.0 as i16) as f32 / s, ((lp.0 >> 16) as i16) as f32 / s)
}

fn leave(t: &mut Toast) {
    if !t.leaving {
        t.leaving = true;
        unsafe {
            let _ = KillTimer(Some(t.hwnd), T_LIFE);
            SetTimer(Some(t.hwnd), T_ANIM, 16, None);
        }
    }
}

/// 关掉所有提示（退出时用）
pub fn clear(a: &mut App) {
    for t in a.toasts.drain(..) {
        unsafe {
            let _ = DestroyWindow(t.hwnd);
        }
    }
}

pub unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_NCHITTEST => return LRESULT(HTCLIENT as isize),
            WM_ERASEBKGND => return LRESULT(1),
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            _ => {}
        }
        let a = app();
        let Some(i) = a.toasts.iter().position(|t| t.hwnd == hwnd) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                let mut t = a.toasts.remove(i);
                draw(a, &mut t);
                a.toasts.insert(i, t);
            }
            WM_TIMER if wp.0 == T_ANIM => {
                let t = &mut a.toasts[i];
                if t.leaving {
                    t.frame = t.frame.saturating_sub(1);
                } else {
                    t.frame = (t.frame + 1).min(FRAMES);
                }
                let (done_in, done_out) = (!t.leaving && t.frame == FRAMES, t.leaving && t.frame == 0);
                if done_in {
                    let _ = KillTimer(Some(hwnd), T_ANIM);
                }
                if done_out {
                    let t = a.toasts.remove(i);
                    let _ = DestroyWindow(t.hwnd);
                    pump(a);
                }
                layout(a);
            }
            WM_TIMER if wp.0 == T_LIFE => leave(&mut a.toasts[i]),
            WM_MOUSEMOVE => {
                let t = &mut a.toasts[i];
                if !t.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    t.tracking = true;
                    // 鼠标停在卡片上：不走倒计时
                    let _ = KillTimer(Some(hwnd), T_LIFE);
                }
                let hc = on_close(hwnd, lp);
                if hc != t.hover_close {
                    t.hover_close = hc;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            WM_MOUSELEAVE => {
                let t = &mut a.toasts[i];
                t.tracking = false;
                t.hover_close = false;
                let _ = InvalidateRect(Some(hwnd), None, false);
                if !t.leaving {
                    SetTimer(Some(hwnd), T_LIFE, 3000, None);
                }
            }
            WM_LBUTTONUP => {
                // 卡片也不抢焦点，菜单开着时一并关掉
                crate::menu::close(a);
                let close_only = on_close(hwnd, lp);
                let pid = a.toasts[i].note.pid;
                leave(&mut a.toasts[i]);
                if !close_only {
                    a.toast_clicked(pid);
                }
            }
            WM_DESTROY => {}
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}
