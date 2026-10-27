#![windows_subsystem = "windows"]
//! Claude 状态磁贴：桌面上一块小窗，显示正在跑的会话（CLI 和桌面版）、5 小时 / 每周额度、每天的 Token 用量热力图。
//!
//! 参数：--claude-dir <目录> 代替 ~/.claude；--data <目录> 代替 %APPDATA%\ClaudeTile（设置和用量历史）；
//! --usage-file <文件> 用文件内容代替额度接口的返回；--dump <文件> 不开窗口，把读到的数据写成文本；
//! --top 置顶；--light / --dark 强制主题；--lang zh|en 强制语言（后几个是截图走查用的）。

mod accounts;
mod config;
mod gfx;
mod icon;
mod json;
mod menu;
mod quota;
mod reset;
mod sessions;
mod sync;
mod sys;
mod time;
mod toast;
mod usage;
mod waker;
mod view;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use config::{Config, DEFAULT_HEIGHTS, en, tr};
use icon::Tone;
use quota::{Fetch, Kind};
use sessions::{Session, State};
use sys::wide;
use usage::{Days, SessionInfo};
use view::{Fonts, Hit, Theme};

const CLASS: PCWSTR = w!("ClaudeTileMain");
const WM_APP_TRAY: u32 = WM_APP + 1;
/// 会话目录有变化
const WM_APP_SESS: u32 = WM_APP + 2;
/// ~/.claude/projects 有变化（对话记录在写）
const WM_APP_PROJ: u32 = WM_APP + 3;
/// 后台线程有结果
const WM_APP_WAKE: u32 = WM_APP + 4;
const T_SESS: usize = 1;
const T_SCAN: usize = 2;
const T_TICK: usize = 3;
/// 额度多久查一次（秒）：正常 2 分钟；失败 1 分钟后重试，被限流 5 分钟
const POLL_OK: i64 = 120;
const POLL_ERR: i64 = 60;
const POLL_SLOW: i64 = 300;
const EDGE: f32 = 6.0;
const DEFAULT_W: f32 = 380.0;

enum Msg {
    Scan(Days, HashMap<String, SessionInfo>),
    Quota(Fetch),
    Sync(Option<sync::Sync>),
    /// 其他账号的额度
    Acct(PathBuf, Fetch),
    /// 打卡完成：账号目录（None = 当前账号）、成没成功
    Pinged(Option<PathBuf>, bool),
    /// 添加账号的命令行窗口关了
    Login(PathBuf),
    Waker(std::result::Result<waker::Status, String>),
    Predictions(reset::Feeds),
    Identity(std::result::Result<Vec<waker::Identity>, String>),
}

pub struct App {
    claude: PathBuf,
    data: PathBuf,
    fake: Option<PathBuf>,
    pub cfg: Config,
    hwnd: HWND,
    d2d: ID2D1Factory1,
    pub gfx: gfx::Gfx,
    pub dw: IDWriteFactory,
    pub fonts: Fonts,
    pub theme: Theme,
    force_dark: Option<bool>,
    surf: Option<gfx::Surface>,
    /// 合并了历史文件的每日用量
    pub days: Days,
    days_dirty: bool,
    pub info: HashMap<String, SessionInfo>,
    pub sessions: Vec<Session>,
    pub desk_titles: HashMap<String, String>,
    pub quota: Fetch,
    /// 最近一次拿到结果的时间，0 = 还没有
    pub quota_at: i64,
    /// 最近一次刷新失败，显示的是之前的数据
    pub stale: bool,
    next_poll: i64,
    fetching: bool,
    pub waker: Option<std::result::Result<waker::Status, String>>,
    waker_fetching: bool,
    waker_next_poll: i64,
    pub predictions: Option<reset::Feeds>,
    predictions_fetching: bool,
    predictions_next_poll: i64,
    pub identity: Option<std::result::Result<Vec<waker::Identity>, String>>,
    identity_fetching: bool,
    identity_next_poll: i64,
    pub plan: String,
    /// 5 小时额度的 (时间, 百分比) 样本，估算用完时间；重置时间变了就清空
    pub samples: Vec<(i64, f64)>,
    samples_reset: Option<i64>,
    /// 已提醒过的额度（种类, 重置时间），每个周期只提醒一次
    alerted: HashSet<(String, i64)>,
    pub menu: Option<Box<menu::Menu>>,
    pub toasts: Vec<toast::Toast>,
    /// 还没弹出来的提示（最多同时显示 3 张，打扰模式下先攒着）
    pub notes: VecDeque<toast::Note>,
    pub hits: Vec<(D2D_RECT_F, Hit)>,
    pub hover: Option<Hit>,
    /// 三个框内容的自然高度（上一帧量的）、滚动位置、实际的 (顶, 高)
    pub nat: [f32; 3],
    pub scroll: [f32; 3],
    boxes: [(f32, f32); 3],
    /// 正在拖第几个框下方的拖动条；按下时的鼠标屏幕 y 和框高
    pub dragging: Option<usize>,
    drag0: (i32, f32),
    /// 接口变了的通知本次运行只弹一次
    changed_noted: bool,
    /// 热力图里点选的日（或按周时那一周的周一）
    pub sel: Option<i32>,
    tracking: bool,
    /// 按下左键：屏幕坐标、窗口位置、是否拖过阈值
    press: Option<(POINT, RECT, bool)>,
    tray_icon: HICON,
    tray_key: (i32, Tone),
    taskbar_created: u32,
    scan_tx: Sender<()>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    scan_pending: bool,
    /// ~/.claude 的同步状态，None = 这台机器不用 sync.sh
    pub sync: Option<sync::Sync>,
    /// sync.log 上次读时的修改时间
    sync_mtime: Option<std::time::SystemTime>,
    /// Claude Code 的全局配置（~/.claude.json），当前账号的 oauthAccount 在这里
    global: PathBuf,
    /// 当前账号的邮箱和其他登录过的账号
    pub email: String,
    pub accounts: Vec<accounts::Account>,
    /// 当前账号正在打卡、下次可以打卡的时间
    pinging: bool,
    ping_at: i64,
    /// 各账号（按邮箱）上次打卡成功的时间，不存盘
    pinged: HashMap<String, i64>,
}

static mut APP: *mut App = std::ptr::null_mut();

/// 单线程 UI，全局状态只在主线程上访问
pub fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

fn arg(args: &[String], k: &str) -> Option<PathBuf> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(PathBuf::from)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
}

fn appdata() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
}

/// 会话目录：~/.claude/sessions；用默认目录时再加上 Orca 各账号档的
/// （Orca 起 claude 时设了 CLAUDE_CONFIG_DIR=%APPDATA%\orca\claude-profiles\<档>\home，projects 链回了 ~/.claude，sessions 没有）
fn session_dirs(claude: &Path) -> Vec<PathBuf> {
    let mut v = vec![claude.join("sessions")];
    if claude == home().join(".claude") {
        let orca = std::fs::read_dir(appdata().join("orca").join("claude-profiles")).into_iter().flatten().flatten();
        v.extend(orca.map(|e| e.path().join("home").join("sessions")).filter(|d| d.is_dir()));
    }
    v
}

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

fn lparam_pt(lp: LPARAM) -> POINT {
    POINT { x: lp.0 as i16 as i32, y: (lp.0 >> 16) as i16 as i32 }
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1;
    for (d, c) in dst.iter_mut().take(n).zip(s.encode_utf16().chain([0])) {
        *d = c;
    }
}

fn work_area(mon: HMONITOR) -> RECT {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(mon, &mut mi);
    }
    mi.rcWork
}

/// 矩形整个挪进工作区，返回新的左上角；比工作区还大时贴左上
fn fit(r: &RECT, wa: RECT) -> (i32, i32) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (r.left.min(wa.right - w).max(wa.left), r.top.min(wa.bottom - h).max(wa.top))
}

fn keep_on_screen(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
        let (x, y) = fit(&r, work_area(MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST)));
        if (x, y) != (r.left, r.top) {
            let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

impl App {
    fn cfg_path(&self) -> PathBuf {
        self.data.join("config.txt")
    }

    fn save_cfg(&self) {
        let _ = std::fs::create_dir_all(&self.data);
        let _ = std::fs::write(self.cfg_path(), self.cfg.to_text());
    }

    fn save_days(&mut self) {
        if std::mem::take(&mut self.days_dirty) {
            let _ = std::fs::create_dir_all(&self.data);
            let _ = std::fs::write(self.data.join("history.txt"), usage::to_text(&self.days));
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn load_theme(&mut self) {
        let sys_dark = self.force_dark.unwrap_or_else(|| sys::reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme")) != Some(1));
        // 透明度低于 50% 时底色太薄，深色字在深色壁纸上看不清：不管系统主题，都用桌面图标那样的白字加阴影；同时关掉模糊，透出壁纸。
        // 菜单和提示卡片跟着磁贴实际用的深浅色走
        let o = self.cfg.opacity.clamp(menu::OPACITY_MIN, 100) as f32 / 100.0;
        let dark = sys_dark || o < 0.5;
        self.theme = Theme::new(dark, o);
        unsafe {
            let d = BOOL::from(dark);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &d as *const _ as _, 4);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
            // 系统亚克力（DWMSBT_TRANSIENTWINDOW）在窗口未激活时退成纯色，磁贴几乎总是未激活，
            // 所以用 SetWindowCompositionAttribute 的亚克力模糊：未公开，但从 Win10 起一直可用（和 Nookery 一样）
            #[repr(C)]
            struct Accent {
                state: u32,
                flags: u32,
                tint: u32,
                anim: u32,
            }
            #[repr(C)]
            struct Data {
                attr: u32,
                data: *mut Accent,
                size: usize,
            }
            type Swca = unsafe extern "system" fn(HWND, *mut Data) -> BOOL;
            if let Some(f) = GetProcAddress(GetModuleHandleW(w!("user32.dll")).unwrap_or_default(), s!("SetWindowCompositionAttribute")) {
                let f: Swca = std::mem::transmute(f);
                // 4 = ACCENT_ENABLE_ACRYLICBLURBEHIND；染色交给 D2D 画的底色
                let mut accent = Accent { state: if o >= 0.5 { 4 } else { 0 }, flags: 0, tint: 0x01000000, anim: 0 };
                let mut d = Data { attr: 19, data: &mut accent, size: size_of::<Accent>() };
                let _ = f(self.hwnd, &mut d);
            }
        }
        self.invalidate();
    }

    fn paint(&mut self) {
        unsafe {
            let _ = ValidateRect(Some(self.hwnd), None);
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            if self.surf.is_none() {
                self.surf = self.gfx.surface(self.hwnd, rc.right as u32, rc.bottom as u32).ok();
            }
            let Some(surf) = self.surf.clone() else { return };
            let s = scale(self.hwnd);
            if self.gfx.bind(&surf, s * 96.0).is_err() {
                return self.reset_gfx();
            }
            let mut wr = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut wr);
            let wa = work_area(MonitorFromRect(&wr, MONITOR_DEFAULTTONEAREST));
            self.gfx.dc.BeginDraw();
            let fr = view::draw(self, rc.right as f32 / s, rc.bottom as f32 / s, (wa.bottom - wa.top) as f32 / s);
            if self.gfx.dc.EndDraw(None, None).is_err() {
                return self.reset_gfx();
            }
            let _ = self.gfx.present(&surf);
            self.hits = fr.hits;
            self.boxes = fr.boxes;
            // 框高按上一帧量出的内容高度定：内容变了再画一次
            if fr.nat != self.nat {
                self.nat = fr.nat;
                self.invalidate();
            }
            for i in 0..3 {
                self.scroll[i] = self.scroll[i].min((self.nat[i] - self.boxes[i].1).max(0.0)).max(0.0);
            }
            // 高度跟着内容走。顶边以用户摆的位置为准，放不下才临时往上挪，内容变矮后回到原处；拖动窗口途中不动位置
            let want = (fr.h * s).ceil() as i32;
            if want != rc.bottom {
                let (x, y) = fit(&RECT { left: wr.left, top: self.cfg.pos[1], right: wr.right, bottom: self.cfg.pos[1] + want }, wa);
                let flags = if self.press.is_some() { SWP_NOMOVE } else { SET_WINDOW_POS_FLAGS(0) };
                let _ = SetWindowPos(self.hwnd, None, x, y, rc.right, want, flags | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    /// 设备丢失（驱动更新、显卡重置）后重建
    fn reset_gfx(&mut self) {
        if let Ok(g) = gfx::Gfx::new(&self.d2d) {
            self.gfx = g;
            self.surf = None;
            self.invalidate();
        }
    }

    fn hit_at(&self, pt: POINT) -> Option<Hit> {
        let s = scale(self.hwnd);
        let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
        self.hits.iter().find(|(r, _)| view::inside(r, x, y)).map(|(_, h)| *h)
    }

    fn click(&mut self, h: Hit) {
        match h {
            Hit::Session(pid) => {
                if !sys::focus(pid) {
                    unsafe { MessageBeep(MB_ICONWARNING).ok() };
                }
            }
            Hit::Day(d) | Hit::Week(d) => self.sel = if self.sel == Some(d) { None } else { Some(d) },
            Hit::Toggle(w) => {
                if self.cfg.weekly != w {
                    self.cfg.weekly = w;
                    self.sel = None;
                    self.save_cfg();
                }
            }
            Hit::Cache => {
                self.cfg.cache_read = !self.cfg.cache_read;
                self.save_cfg();
            }
            Hit::Account(i) => self.switch_to(i),
            Hit::Quota | Hit::Remote => { self.fetch_quota(); self.fetch_identity(); }
            Hit::Waker => { self.menu_action(menu::Id::Waker); }
            Hit::Heatmap => { self.menu_action(menu::Id::Heatmap); }
            Hit::Projects => { self.menu_action(menu::Id::Projects); }
            Hit::Prediction(i) => unsafe {
                let url = wide(reset::EMBEDS[i]);
                let _ = ShellExecuteW(Some(self.hwnd), w!("open"), PCWSTR(url.as_ptr()), None, None, SW_SHOWNORMAL);
            },
            Hit::Pin => self.set_topmost(!self.cfg.topmost),
            Hit::Split(_) => {}
        }
        self.invalidate();
    }

    fn request_scan(&mut self) {
        let _ = self.scan_tx.send(());
    }

    fn fetch_quota(&mut self) {
        if self.fetching {
            return;
        }
        self.fetching = true;
        let (claude, fake, tx, hwnd) = (self.claude.clone(), self.fake.clone(), self.tx.clone(), self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let r = quota::fetch(&claude, quota::proxy(&claude).as_deref(), fake.as_deref());
            let _ = tx.send(Msg::Quota(r));
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// 在后台线程里跑 f，结果发回主线程
    fn bg(&self, f: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, hwnd) = (self.tx.clone(), self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let _ = tx.send(f());
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// 重读当前账号的邮箱和其他账号的列表（已有账号的额度留着），新出现的账号马上查额度
    fn reload_accounts(&mut self) {
        self.email = accounts::email(&self.global).unwrap_or_default();
        let mut old = std::mem::take(&mut self.accounts);
        for (dir, email) in accounts::load(&self.data.join("accounts")) {
            if email == self.email {
                continue;
            }
            match old.iter().position(|a| a.dir == dir) {
                Some(i) => self.accounts.push(accounts::Account { email, ..old.swap_remove(i) }),
                None => self.accounts.push(accounts::Account { dir, email, quota: Fetch::Err(String::new()), next_poll: 0, next_ping: 0, fetching: false, pinging: false }),
            }
        }
        for i in 0..self.accounts.len() {
            if self.accounts[i].next_poll == 0 {
                self.fetch_acct(i);
            }
        }
    }

    fn fetch_acct(&mut self, i: usize) {
        let (proxy, fake) = (quota::proxy(&self.claude), self.fake.clone());
        let a = &mut self.accounts[i];
        if std::mem::replace(&mut a.fetching, true) {
            return;
        }
        let dir = a.dir.clone();
        self.bg(move || Msg::Acct(dir.clone(), quota::fetch(&dir, proxy.as_deref(), fake.as_deref())));
    }

    /// 打卡；dir 为 None 是当前账号
    fn ping(&self, dir: Option<PathBuf>) {
        let (exe, proxy) = (accounts::claude_exe(&home()), quota::proxy(&self.claude));
        // 当前账号用默认目录时不设 CLAUDE_CONFIG_DIR，不然 Claude Code 会在 ~/.claude 里另起一份全局配置
        let d = dir.clone().or_else(|| (self.claude != home().join(".claude")).then(|| self.claude.clone()));
        self.bg(move || Msg::Pinged(dir, accounts::ping(&exe, d.as_deref(), proxy.as_deref())));
    }

    /// 开着自动打卡时，5 小时窗口没在跑的账号各打一次卡
    fn auto_ping(&mut self) {
        let now = time::now();
        if !self.cfg.ping || waker::linked(&self.data) {
            return;
        }
        if !self.pinging && now >= self.ping_at && accounts::idle(&self.quota, now) {
            self.pinging = true;
            self.ping(None);
        }
        for i in 0..self.accounts.len() {
            let a = &mut self.accounts[i];
            if !a.pinging && now >= a.next_ping && accounts::idle(&a.quota, now) {
                a.pinging = true;
                let d = a.dir.clone();
                self.ping(Some(d));
            }
        }
    }

    fn switch_to(&mut self, i: usize) {
        // 打卡的 claude 进程可能正在刷新凭证，等它跑完再切
        let busy = self.pinging || self.accounts.iter().any(|a| a.pinging);
        if busy || accounts::switch(&self.claude, &self.global, &self.data.join("accounts"), &self.accounts[i].dir).is_err() {
            unsafe { MessageBeep(MB_ICONWARNING).ok() };
            return;
        }
        // 两个账号的额度先对调着显示，再各查一次
        let (old_email, q) = (self.email.clone(), std::mem::replace(&mut self.accounts[i].quota, Fetch::Err(String::new())));
        let old = std::mem::replace(&mut self.quota, q);
        self.quota_at = if matches!(self.quota, Fetch::Ok(_)) { time::now() } else { 0 };
        (self.stale, self.ping_at, self.samples_reset) = (false, 0, None);
        self.samples.clear();
        self.plan = quota::creds(&self.claude).map(|c| c.plan).unwrap_or_default();
        self.reload_accounts();
        if let Some(a) = self.accounts.iter_mut().find(|a| a.email == old_email) {
            a.quota = old;
        }
        self.fetch_quota();
        self.update_tray();
    }

    fn on_quota(&mut self, r: Fetch) {
        self.fetching = false;
        let now = time::now();
        self.plan = quota::creds(&self.claude).map(|c| c.plan).unwrap_or_default();
        self.next_poll = now
            + match &r {
                Fetch::Ok(_) => POLL_OK,
                Fetch::Err(e) if e.contains("429") => POLL_SLOW,
                Fetch::Changed(_) => POLL_SLOW,
                _ => POLL_ERR,
            };
        match (&r, &self.quota) {
            // 刷新失败时继续显示上一次的数，标出来
            (Fetch::Err(_), Fetch::Ok(_)) => self.stale = true,
            _ => {
                self.stale = false;
                self.quota_at = now;
                self.quota = r;
            }
        }
        if matches!(self.quota, Fetch::Changed(_)) && !std::mem::replace(&mut self.changed_noted, true) {
            self.notify(
                Tone::Error,
                "\u{E895}",
                tr("额度接口已变更", "Usage API changed"),
                tr("当前版本读不了额度，请更新 ClaudeTile。会话和 Token 统计不受影响。", "This version can't read your usage limits. Please update ClaudeTile; sessions and tokens still work."),
                None,
            );
        }
        if let Fetch::Ok(ws) = &self.quota {
            let ws = ws.clone();
            if let Some(w) = ws.iter().find(|w| w.kind == Kind::Session) {
                if self.samples_reset != w.resets {
                    self.samples.clear();
                    self.samples_reset = w.resets;
                }
                if !self.stale {
                    self.samples.push((now, w.pct));
                    self.samples.retain(|(t, _)| now - t <= 7200);
                }
            }
            self.check_alerts(&ws, now);
        }
        // 在 Claude Code 里 /login 换了账号也跟上
        self.reload_accounts();
        self.auto_ping();
        self.update_tray();
        self.invalidate();
    }

    /// 额度到了提醒线、或按当前速度半小时内会用完 5 小时额度：弹一次系统通知，每个周期只弹一次
    fn check_alerts(&mut self, ws: &[quota::Window], now: i64) {
        if self.cfg.alert == 0 {
            return;
        }
        for w in ws {
            let key = (view::quota_label(&w.kind), w.resets.unwrap_or(0));
            if w.pct >= self.cfg.alert as f64 && self.alerted.insert(key.clone()) {
                let reset = w.resets.map(|t| view::fmt_dur(t - now)).unwrap_or_default();
                let text = if en() { format!("{} usage at {:.0}%, resets in {reset}", key.0, w.pct) } else { format!("{}额度已用 {:.0}%，{reset}后重置", key.0, w.pct) };
                // Segoe Fluent Icons：E7BA 警告
                self.notify(Tone::Idle, "\u{E7BA}", tr("额度提醒", "Usage alert"), &text, None);
            }
            if w.kind == Kind::Session {
                let eta = quota::eta(&self.samples, now).filter(|t| t - now <= 1800 && w.resets.is_none_or(|r| *t < r));
                if let Some(t) = eta {
                    if self.alerted.insert((format!("{}eta", key.0), key.1)) {
                        let text = if en() { format!("At this pace the 5-hour limit runs out in about {}", view::fmt_dur(t - now)) } else { format!("按当前速度，约 {} 后用完 5 小时额度", view::fmt_dur(t - now)) };
                        self.notify(Tone::Idle, "\u{E7BA}", tr("额度提醒", "Usage alert"), &text, None);
                    }
                }
            }
        }
    }

    fn reload_sessions(&mut self) {
        let new = sessions::load(&session_dirs(&self.claude));
        if new.iter().any(|s| s.desktop) {
            self.desk_titles = sessions::desktop_titles(&appdata().join("Claude").join("claude-code-sessions"));
        }
        // 新进入“等你处理”的会话：弹通知，点通知切过去
        for s in &new {
            if let State::Waiting(wf) = &s.state {
                if !self.sessions.iter().any(|o| o.pid == s.pid && o.state == s.state) {
                    let title = self.title_of(s);
                    // E8BD 消息
                    self.notify(Tone::Waiting, "\u{E8BD}", view::waiting_text(wf), &format!("{title} · {}", s.project), Some(s.pid));
                }
            }
        }
        // 有会话跑完一轮：额度多半变了，提前刷新（至少隔 30 秒）
        let finished = self.sessions.iter().any(|o| o.state != State::Idle && new.iter().any(|n| n.pid == o.pid && n.state == State::Idle));
        self.sessions = new;
        if finished && time::now() - self.quota_at > 30 {
            self.fetch_quota();
        }
        self.update_tray();
        self.invalidate();
    }

    fn title_of(&self, s: &Session) -> String {
        self.desk_titles.get(&s.id).cloned().or_else(|| self.info.get(&s.id).and_then(|i| i.title.clone())).unwrap_or_else(|| s.name.clone())
    }

    fn session_pct(&self) -> Option<f64> {
        match &self.quota {
            Fetch::Ok(ws) => ws.iter().find(|w| w.kind == Kind::Session).map(|w| w.pct),
            _ => None,
        }
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: 1, uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP, uCallbackMessage: WM_APP_TRAY, hIcon: self.tray_icon, ..Default::default() };
        let mut tip = "Claude".to_string();
        if let Fetch::Ok(ws) = &self.quota {
            for w in ws.iter().take(2) {
                tip += &format!(" · {} {:.0}%", view::quota_label(&w.kind), w.pct);
            }
        }
        let busy = self.sessions.iter().filter(|s| s.state != State::Idle).count();
        if busy > 0 {
            tip += &if en() { format!("\n{busy} active session(s)") } else { format!("\n{busy} 个会话进行中") };
        }
        copy_wide(&mut nid.szTip, &tip);
        nid
    }

    fn tray(&self, msg: NOTIFY_ICON_MESSAGE) {
        unsafe {
            let _ = Shell_NotifyIconW(msg, &self.tray_data());
        }
    }

    /// 托盘图标按 5 小时额度和“有会话等你”重画；没变就不动
    fn update_tray(&mut self) {
        let pct = self.session_pct();
        // 等你确认 > 异常（登录过期、接口变了、5 小时额度用到 90%）> 有会话在跑 > 空闲
        let tone = if self.sessions.iter().any(|s| matches!(s.state, State::Waiting(_))) {
            Tone::Waiting
        } else if matches!(self.quota, Fetch::Expired | Fetch::NoLogin | Fetch::Changed(_)) || pct.is_some_and(|p| p >= 90.0) {
            Tone::Error
        } else if self.sessions.iter().any(|s| s.state == State::Busy) {
            Tone::Busy
        } else {
            Tone::Idle
        };
        let key = (pct.map_or(-1, |p| p.round() as i32), tone);
        if key != self.tray_key {
            let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
            if let Some(ic) = icon::make(&self.d2d, &self.dw, size, pct, tone) {
                let old = std::mem::replace(&mut self.tray_icon, ic);
                self.tray_key = key;
                self.tray(NIM_MODIFY);
                unsafe {
                    let _ = DestroyIcon(old);
                }
                return;
            }
        }
        self.tray(NIM_MODIFY);
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.cfg.hidden = hidden;
        self.save_cfg();
        unsafe {
            let _ = ShowWindow(self.hwnd, if hidden { SW_HIDE } else { SW_SHOWNOACTIVATE });
        }
    }

    fn set_topmost(&mut self, on: bool) {
        self.cfg.topmost = on;
        self.save_cfg();
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn save_pos(&mut self) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut r);
        }
        self.cfg.pos = [r.left, r.top, r.right - r.left];
        self.save_cfg();
    }

    /// 在鼠标处弹出右键菜单（磁贴和托盘共用）
    fn menu(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        menu::show(self, pt);
    }

    /// 菜单里点了某项；返回 true 表示菜单要关
    pub fn menu_action(&mut self, id: menu::Id) -> bool {
        use menu::Id;
        match id {
            Id::Heatmap | Id::Projects => {
                if id == Id::Heatmap { self.cfg.heatmap = !self.cfg.heatmap; } else { self.cfg.projects = !self.cfg.projects; }
                self.scroll[2] = 0.0;
                self.cfg.heights[2] = 0.0;
                self.save_cfg();
            }
            Id::Opacity => return false,
            Id::Top => self.set_topmost(!self.cfg.topmost),
            Id::Ping => {
                if waker::linked(&self.data) {
                    let _ = waker::configure(&self.data);
                    return true;
                }
                self.cfg.ping = !self.cfg.ping;
                self.save_cfg();
                self.auto_ping();
            }
            Id::AddAccount => {
                let (exe, proxy) = (accounts::claude_exe(&home()), quota::proxy(&self.claude));
                let dir = self.data.join("accounts").join(format!("new-{}", time::now()));
                self.bg(move || {
                    accounts::login(&exe, &dir, proxy.as_deref());
                    Msg::Login(dir)
                });
            }
            Id::Auto => sys::set_autostart(!sys::autostart()),
            Id::Alert(v) => {
                self.cfg.alert = v;
                self.save_cfg();
            }
            Id::Tz(v) => {
                self.cfg.tz = v;
                self.save_cfg();
            }
            Id::Lang(en) => {
                self.cfg.en = Some(en);
                config::set_lang(self.cfg.en);
                self.save_cfg();
                self.update_tray();
            }
            Id::Refresh => {
                self.fetch_quota();
                self.fetch_waker();
                self.fetch_predictions();
                self.fetch_identity();
                for i in 0..self.accounts.len() { self.fetch_acct(i); }
                self.request_scan();
                self.reload_sessions();
            }
            Id::Layout => {
                self.cfg.heights = DEFAULT_HEIGHTS;
                self.scroll = [0.0; 3];
                self.save_cfg();
            }
            Id::Hide => self.set_hidden(!self.cfg.hidden),
            Id::Exit => {
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
                return true;
            }
            Id::Waker => {
                if let Err(error) = waker::configure(&self.data) {
                    unsafe {
                        let text = wide(&error);
                        let _ = MessageBoxW(Some(self.hwnd), PCWSTR(text.as_ptr()), w!("ClaudeTile"), MB_OK | MB_ICONINFORMATION);
                    }
                }
                self.fetch_waker();
                return true;
            }
        }
        self.invalidate();
        matches!(id, Id::AddAccount | Id::Refresh | Id::Layout | Id::Hide)
    }

    /// 改透明度（拖滑块时实时生效，松手再存盘）
    pub fn set_opacity(&mut self, v: u32) {
        self.cfg.opacity = v;
        self.load_theme();
    }

    /// 弹一张右下角提示；pid 是点卡片时要切过去的会话
    fn notify(&mut self, tone: Tone, glyph: &'static str, title: &str, text: &str, pid: Option<u32>) {
        self.notes.push_back(toast::Note { title: title.to_string(), text: text.to_string(), tone, glyph, pid });
        toast::pump(self);
    }

    /// 点了提示卡片：等你处理的会话切过去，其他显示磁贴
    pub fn toast_clicked(&mut self, pid: Option<u32>) {
        if !pid.is_some_and(sys::focus) {
            self.set_hidden(false);
        }
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Identity(identity) => {
                    self.identity_fetching = false;
                    self.identity_next_poll = time::now() + 300;
                    self.identity = Some(identity);
                    self.invalidate();
                }
                Msg::Predictions(feeds) => {
                    self.predictions_fetching = false;
                    self.predictions_next_poll = time::now() + 300;
                    self.predictions = Some(feeds);
                    self.invalidate();
                }
                Msg::Waker(state) => {
                    self.waker_fetching = false;
                    self.waker_next_poll = time::now() + 60;
                    self.waker = Some(state);
                    self.invalidate();
                }
                Msg::Scan(days, info) => {
                    usage::merge_max(&mut self.days, &days);
                    self.days_dirty = true;
                    let first = self.info.is_empty();
                    self.info = info;
                    // 启动时等第一次扫描拿到会话标题再读会话，否则“等你确认”的提示里只有会话名
                    if first {
                        self.reload_sessions();
                    }
                    self.invalidate();
                }
                Msg::Quota(r) => self.on_quota(r),
                Msg::Sync(s) => {
                    self.sync = s;
                    self.invalidate();
                }
                Msg::Acct(dir, q) => {
                    if let Some(a) = self.accounts.iter_mut().find(|a| a.dir == dir) {
                        a.fetching = false;
                        a.next_poll = time::now() + if matches!(q, Fetch::Ok(_)) { POLL_SLOW } else { POLL_ERR };
                        // 刷新失败时继续显示上一次的数
                        if !matches!((&q, &a.quota), (Fetch::Err(_), Fetch::Ok(_))) {
                            a.quota = q;
                        }
                        self.auto_ping();
                        self.invalidate();
                    }
                }
                Msg::Pinged(dir, ok) => {
                    // 成功后等 10 分钟让额度接口反映出来，失败 15 分钟后再试
                    let now = time::now();
                    let email = match &dir {
                        None => Some(self.email.clone()),
                        Some(d) => self.accounts.iter().find(|a| a.dir == *d).map(|a| a.email.clone()),
                    };
                    if let (true, Some(e)) = (ok, email) {
                        self.pinged.insert(e, now);
                    }
                    let next = now + if ok { 600 } else { 900 };
                    match dir {
                        None => {
                            (self.pinging, self.ping_at) = (false, next);
                            self.fetch_quota();
                        }
                        Some(d) => {
                            if let Some(i) = self.accounts.iter().position(|a| a.dir == d) {
                                (self.accounts[i].pinging, self.accounts[i].next_ping) = (false, next);
                                self.fetch_acct(i);
                            }
                        }
                    }
                }
                Msg::Login(dir) => {
                    // 没登录成功、或登的是当前账号就删掉这个目录；登的是列表里已有的账号就换掉旧目录
                    let e = dir.join(".credentials.json").is_file().then(|| accounts::email(&dir.join(".claude.json"))).flatten();
                    match e {
                        Some(e) if e != self.email => {
                            for a in self.accounts.iter().filter(|a| a.email == e) {
                                let _ = std::fs::remove_dir_all(&a.dir);
                            }
                        }
                        _ => {
                            let _ = std::fs::remove_dir_all(&dir);
                        }
                    }
                    self.reload_accounts();
                    self.invalidate();
                }
            }
        }
    }

    fn fetch_predictions(&mut self) {
        if std::mem::replace(&mut self.predictions_fetching, true) { return; }
        let proxy = quota::proxy(&self.claude);
        self.bg(move || Msg::Predictions(reset::fetch(proxy.as_deref())));
    }

    fn fetch_identity(&mut self) {
        if !waker::linked(&self.data) { self.identity_next_poll = time::now() + 300; return; }
        if std::mem::replace(&mut self.identity_fetching, true) { return; }
        self.identity = None;
        let data = self.data.clone();
        self.bg(move || Msg::Identity(waker::identity(&data)));
    }

    fn fetch_waker(&mut self) {
        if !waker::linked(&self.data) {
            self.waker_next_poll = time::now() + 60;
            return;
        }
        if std::mem::replace(&mut self.waker_fetching, true) {
            return;
        }
        // A linked VPS owns wake-ups, including while paused or unreachable.
        if self.cfg.ping {
            self.cfg.ping = false;
            self.save_cfg();
        }
        let data = self.data.clone();
        self.bg(move || Msg::Waker(waker::fetch(&data)));
    }

    /// sync.log 变了（同步跑完）或到了计划任务的下次运行时间，就在后台重读同步状态
    fn check_sync(&mut self) {
        let m = std::fs::metadata(self.claude.join("sync.log")).and_then(|m| m.modified()).ok();
        let due = self.sync.as_ref().and_then(|s| s.next).is_some_and(|n| time::now() >= n);
        if m == self.sync_mtime && !due {
            return;
        }
        self.sync_mtime = m;
        let (claude, tx, hwnd) = (self.claude.clone(), self.tx.clone(), self.hwnd.0 as usize);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Sync(sync::load(&claude)));
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        });
    }

    fn tick(&mut self) {
        if time::now() >= self.predictions_next_poll {
            self.fetch_predictions();
        }
        if time::now() >= self.identity_next_poll {
            self.fetch_identity();
        }
        if time::now() >= self.waker_next_poll {
            self.fetch_waker();
        }
        toast::pump(self);
        self.reload_sessions();
        if time::now() >= self.next_poll {
            self.fetch_quota();
        }
        for i in 0..self.accounts.len() {
            if time::now() >= self.accounts[i].next_poll {
                self.fetch_acct(i);
            }
        }
        self.check_sync();
        self.save_days();
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // 整个窗口都是客户区，没有系统边框
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_ERASEBKGND => return LRESULT(1),
            _ => {}
        }
        if APP.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let a = app();
        match msg {
            WM_PAINT => a.paint(),
            WM_SIZE => {
                if let Some(s) = &a.surf {
                    a.gfx.resize(s, (lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                }
                a.invalidate();
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                a.surf = None;
                a.save_pos();
            }
            // 只能左右拖边改宽度，高度跟着内容
            WM_NCHITTEST => {
                let p = lparam_pt(lp);
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                let e = (EDGE * scale(hwnd)) as i32;
                let code = if p.x < r.left + e {
                    HTLEFT
                } else if p.x >= r.right - e {
                    HTRIGHT
                } else {
                    HTCLIENT
                };
                return LRESULT(code as isize);
            }
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp.0 as *mut MINMAXINFO);
                mm.ptMinTrackSize.x = (view::MIN_W * scale(hwnd)) as i32;
            }
            WM_EXITSIZEMOVE => a.save_pos(),
            // 置顶时点磁贴不抢焦点；不置顶时和普通窗口一样，点了激活、盖到其他窗口上面，点别的程序再被盖住。
            // 菜单开着时也不激活：否则菜单先因失活关掉，这次点击会落到磁贴的按钮上
            WM_MOUSEACTIVATE if a.cfg.topmost || a.menu.is_some() => return LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE => {
                if let Some(i) = a.dragging {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    let nh = a.drag0.1 + (p.y - a.drag0.0) as f32 / scale(hwnd);
                    a.cfg.heights[i] = nh.min(a.nat[i]).max(view::MIN_BOX);
                    a.invalidate();
                    return LRESULT(0);
                }
                if let Some((p0, r0, moved)) = a.press.as_mut() {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    let (dx, dy) = (p.x - p0.x, p.y - p0.y);
                    if !*moved && (dx.abs() > GetSystemMetrics(SM_CXDRAG) || dy.abs() > GetSystemMetrics(SM_CYDRAG)) {
                        *moved = true;
                    }
                    if *moved {
                        // 整个窗口留在鼠标所在显示器的工作区里
                        let r = RECT { left: r0.left + dx, top: r0.top + dy, right: r0.right + dx, bottom: r0.bottom + dy };
                        let (x, y) = fit(&r, work_area(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST)));
                        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                    return LRESULT(0);
                }
                if !a.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    a.tracking = true;
                }
                let h = a.hit_at(lparam_pt(lp));
                if h != a.hover {
                    a.hover = h;
                    a.invalidate();
                }
            }
            WM_MOUSELEAVE => {
                a.tracking = false;
                if a.hover.take().is_some() {
                    a.invalidate();
                }
            }
            // 磁贴点击不抢焦点，菜单收不到失活消息：菜单开着时，点磁贴只关菜单，不触发磁贴上的操作（和系统菜单一样）
            WM_LBUTTONDOWN if a.menu.is_some() => menu::close(a),
            WM_LBUTTONDOWN => {
                let (mut p, mut r) = (POINT::default(), RECT::default());
                let _ = GetCursorPos(&mut p);
                if let Some(Hit::Split(i)) = a.hit_at(lparam_pt(lp)) {
                    a.dragging = Some(i);
                    a.drag0 = (p.y, a.boxes[i].1);
                    SetCapture(hwnd);
                    return LRESULT(0);
                }
                let _ = GetWindowRect(hwnd, &mut r);
                a.press = Some((p, r, false));
                SetCapture(hwnd);
            }
            WM_LBUTTONUP => {
                if a.dragging.take().is_some() {
                    let _ = ReleaseCapture();
                    a.save_cfg();
                    a.invalidate();
                    return LRESULT(0);
                }
                let pr = a.press.take();
                let _ = ReleaseCapture();
                match pr {
                    Some((_, _, true)) => a.save_pos(),
                    Some(_) => {
                        if let Some(h) = a.hit_at(lparam_pt(lp)) {
                            a.click(h);
                        }
                    }
                    None => {}
                }
            }
            // 拖动中鼠标被别的窗口抢走（弹窗、Alt+Tab）：当作松开
            WM_CAPTURECHANGED if a.dragging.is_some() && lp.0 != hwnd.0 as isize => {
                a.dragging = None;
                a.save_cfg();
                a.invalidate();
            }
            WM_CAPTURECHANGED if a.press.is_some() && lp.0 != hwnd.0 as isize => {
                if a.press.take().is_some_and(|p| p.2) {
                    a.save_pos();
                }
            }
            WM_RBUTTONUP => a.menu(),
            // 拖动条上显示上下箭头
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let _ = ScreenToClient(hwnd, &mut p);
                if a.dragging.is_some() || matches!(a.hit_at(p), Some(Hit::Split(_))) {
                    SetCursor(LoadCursorW(None, IDC_SIZENS).ok());
                    return LRESULT(1);
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            // 滚轮滚动鼠标所在的框
            WM_MOUSEWHEEL => {
                let mut p = lparam_pt(lp);
                let _ = ScreenToClient(hwnd, &mut p);
                let y = p.y as f32 / scale(hwnd);
                if let Some(i) = (0..3).find(|&i| y >= a.boxes[i].0 && y < a.boxes[i].0 + a.boxes[i].1) {
                    let max = (a.nat[i] - a.boxes[i].1).max(0.0);
                    a.scroll[i] = (a.scroll[i] - (wp.0 >> 16) as i16 as f32 / 120.0 * 40.0).clamp(0.0, max);
                    a.invalidate();
                }
            }
            WM_SETTINGCHANGE => a.load_theme(),
            // 分辨率或显示器变了，跑到屏幕外就拉回来
            WM_DISPLAYCHANGE => {
                keep_on_screen(hwnd);
                a.save_pos();
            }
            WM_APP_SESS => {
                SetTimer(Some(hwnd), T_SESS, 300, None);
            }
            WM_APP_PROJ => {
                // 对话记录写个不停时不能每次都重设定时器（会一直推迟），已在等就不管
                if !a.scan_pending {
                    a.scan_pending = true;
                    SetTimer(Some(hwnd), T_SCAN, 1500, None);
                }
            }
            WM_APP_WAKE => a.drain(),
            WM_TIMER => {
                match wp.0 {
                    T_SESS => {
                        let _ = KillTimer(Some(hwnd), T_SESS);
                        a.reload_sessions();
                    }
                    T_SCAN => {
                        let _ = KillTimer(Some(hwnd), T_SCAN);
                        a.scan_pending = false;
                        a.request_scan();
                    }
                    T_TICK => a.tick(),
                    _ => {}
                }
            }
            WM_APP_TRAY => match (lp.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP => a.set_hidden(!a.cfg.hidden),
                WM_RBUTTONUP => a.menu(),
                _ => {}
            },
            WM_DESTROY => {
                menu::close(a);
                toast::clear(a);
                a.save_days();
                a.tray(NIM_DELETE);
                PostQuitMessage(0);
            }
            m if m == a.taskbar_created => a.tray(NIM_ADD),
            _ => return DefWindowProcW(hwnd, msg, wp, lp),
        }
        LRESULT(0)
    }
}

/// --dump <文件>：不开窗口，把读到的数据写成文本（测试和排查用）
fn dump(claude: &Path, fake: Option<&Path>, out: &Path) {
    let t0 = std::time::Instant::now();
    let mut sc = usage::Scanner::new(claude.join("projects"));
    sc.scan();
    let scan_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    sc.scan();
    let mut s = format!("scan {scan_ms} ms, rescan {} ms, days {}\n", t1.elapsed().as_millis(), sc.days.len());
    let today = time::today();
    for d in today - 13..=today {
        let (y, m, dd) = time::civil(d);
        s += &format!("{y}-{m:02}-{dd:02} {:>8} (+cache read {:>8})\n", usage::fmt_tokens(usage::day_total(&sc.days, d, false)), usage::fmt_tokens(usage::day_total(&sc.days, d, true)));
    }
    let (sum, proj, model) = usage::breakdown(&sc.days, today - 6, today, false);
    s += &format!("7d {} | {:?} | {:?}\n", usage::fmt_tokens(sum), &proj[..proj.len().min(6)], model);
    let titles = sessions::desktop_titles(&appdata().join("Claude").join("claude-code-sessions"));
    for x in sessions::load(&session_dirs(claude)) {
        let info = sc.sessions.get(&x.id).cloned().unwrap_or_default();
        s += &format!("session {:?} title={:?} desktop_title={:?} tool={:?}\n", x, info.title, titles.get(&x.id), info.tool);
    }
    s += &format!("plan {:?}\n", quota::creds(claude).map(|c| c.plan));
    s += &format!("quota {:?}\n", quota::fetch(claude, quota::proxy(claude).as_deref(), fake));
    let _ = std::fs::write(out, s);
}

fn main() {
    // 没有控制台，崩溃信息写到 %TEMP%\claudetile-crash.txt
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(std::env::temp_dir().join("claudetile-crash.txt"), format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    let args: Vec<String> = std::env::args().collect();
    let claude = arg(&args, "--claude-dir").unwrap_or_else(|| home().join(".claude"));
    let fake = arg(&args, "--usage-file");
    if let Some(out) = arg(&args, "--dump") {
        return dump(&claude, fake.as_deref(), &out);
    }
    let data = arg(&args, "--data").unwrap_or_else(|| appdata().join("ClaudeTile"));
    unsafe {
        let mutex = wide(&format!("ClaudeTile-{}", data.to_string_lossy().replace('\\', "/").to_lowercase()));
        let _guard = CreateMutexW(None, true, PCWSTR(mutex.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // 程序里没有输入框：关掉输入法。否则右键菜单一拿到焦点，系统就把输入法（如搜狗拼音）加载进本进程，私有内存 +34 MB
        let _ = windows::Win32::UI::Input::Ime::ImmDisableIME(u32::MAX);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let mut cfg = Config::parse(&std::fs::read_to_string(data.join("config.txt")).unwrap_or_default());
        if let Some(l) = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)) {
            cfg.en = Some(l == "en");
        }
        config::set_lang(cfg.en);
        let days = usage::from_text(&std::fs::read_to_string(data.join("history.txt")).unwrap_or_default());

        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("D2D 初始化失败");
        let gfx = gfx::Gfx::new(&d2d).expect("D3D 初始化失败");
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DirectWrite 初始化失败");
        let fonts = Fonts::new(&dw);

        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        type Proc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
        let classes: [(Proc, PCWSTR); 3] = [(wnd_proc, CLASS), (menu::menu_proc, menu::MENU_CLASS), (toast::toast_proc, toast::TOAST_CLASS)];
        for (proc_, class) in classes {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), lpszClassName: class, hCursor: cursor, ..Default::default() };
            RegisterClassW(&wc);
        }
        let topmost = cfg.topmost || args.iter().any(|a| a == "--top");
        cfg.topmost = topmost;
        // 默认放在主屏工作区右上角
        let s = GetDpiForSystem() as f32 / 96.0;
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        if cfg.pos[2] == 0 {
            let w = (DEFAULT_W * s) as i32;
            cfg.pos = [wa.right - w - (24.0 * s) as i32, wa.top + (24.0 * s) as i32, w];
        }
        // 工具窗口：不出现在任务栏和 Alt+Tab 里
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP | if topmost { WS_EX_TOPMOST } else { WINDOW_EX_STYLE(0) },
            CLASS,
            w!("Claude Tile"),
            WS_POPUP | WS_THICKFRAME,
            cfg.pos[0],
            cfg.pos[1],
            cfg.pos[2],
            (400.0 * s) as i32,
            None,
            None,
            None,
            None,
        )
        .expect("创建窗口失败");

        let (scan_tx, scan_rx) = channel::<()>();
        let (tx, rx) = channel::<Msg>();
        let mut a = Box::new(App {
            claude: claude.clone(),
            data,
            fake,
            cfg,
            hwnd,
            d2d,
            gfx,
            dw,
            fonts,
            theme: Theme::new(true, 1.0),
            menu: None,
            toasts: Vec::new(),
            force_dark: if args.iter().any(|a| a == "--light") { Some(false) } else if args.iter().any(|a| a == "--dark") { Some(true) } else { None },
            surf: None,
            days,
            days_dirty: false,
            info: HashMap::new(),
            sessions: Vec::new(),
            desk_titles: HashMap::new(),
            quota: Fetch::Err(String::new()),
            quota_at: 0,
            stale: false,
            next_poll: 0,
            fetching: false,
            waker: None,
            waker_fetching: false,
            waker_next_poll: 0,
            predictions: None,
            predictions_fetching: false,
            predictions_next_poll: 0,
            identity: None,
            identity_fetching: false,
            identity_next_poll: 0,
            plan: String::new(),
            samples: Vec::new(),
            samples_reset: None,
            alerted: HashSet::new(),
            notes: VecDeque::new(),
            hits: Vec::new(),
            hover: None,
            nat: [120.0, DEFAULT_HEIGHTS[1], 400.0],
            scroll: [0.0; 3],
            boxes: [(0.0, 0.0); 3],
            dragging: None,
            drag0: (0, 0.0),
            changed_noted: false,
            sel: None,
            tracking: false,
            press: None,
            tray_icon: HICON::default(),
            tray_key: (i32::MIN, Tone::Idle),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            scan_tx,
            rx,
            tx: tx.clone(),
            scan_pending: false,
            sync: None,
            sync_mtime: None,
            global: accounts::global_json(&claude, &home()),
            email: String::new(),
            accounts: Vec::new(),
            pinging: false,
            ping_at: 0,
            pinged: HashMap::new(),
        });
        APP = &mut *a;
        a.load_theme();
        a.tray_icon = icon::make(&a.d2d, &a.dw, GetSystemMetrics(SM_CXSMICON), None, Tone::Idle).unwrap_or_default();
        a.tray(NIM_ADD);

        // 对话记录在后台线程里扫：第一次全量扫描要几百毫秒，不卡界面
        let (root, hw) = (claude.join("projects"), hwnd.0 as usize);
        std::thread::spawn(move || {
            let mut sc = usage::Scanner::new(root);
            let mut first = true;
            while scan_rx.recv().is_ok() {
                while scan_rx.try_recv().is_ok() {}
                if sc.scan() || std::mem::take(&mut first) {
                    let _ = tx.send(Msg::Scan(sc.days.clone(), sc.sessions.clone()));
                    let _ = PostMessageW(Some(HWND(hw as _)), WM_APP_WAKE, WPARAM(0), LPARAM(0));
                }
            }
        });
        a.request_scan();
        let _ = std::fs::create_dir_all(claude.join("sessions"));
        // 磁贴启动后 Orca 新建的账号档不在监视里，它的会话靠 15 秒一跳刷新
        for d in session_dirs(&claude) {
            sys::watch(d, false, hwnd, WM_APP_SESS);
        }
        sys::watch(claude.join("projects"), true, hwnd, WM_APP_PROJ);
        a.reload_accounts();
        a.fetch_quota();
        a.fetch_waker();
        a.fetch_predictions();
        a.fetch_identity();
        a.check_sync();
        // 15 秒一跳：确认会话进程还在、刷新倒计时、到点查额度、看同步状态、存用量历史
        SetTimer(Some(hwnd), T_TICK, 15_000, None);
        if !a.cfg.hidden {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
