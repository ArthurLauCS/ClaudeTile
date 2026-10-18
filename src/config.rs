//! 设置：%APPDATA%\ClaudeTile\config.txt，每行 键|值。界面语言也存这里（没有外壳扩展要读，不用注册表）。

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::Win32::Globalization::GetUserDefaultUILanguage;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// 窗口左上角和宽度（物理像素）；宽度为 0 表示还没摆过
    pub pos: [i32; 3],
    pub topmost: bool,
    /// None 时跟随系统界面语言
    pub en: Option<bool>,
    /// Token 合计里算不算缓存读取（量是其他几项的几十倍，默认不算）
    pub cache_read: bool,
    /// 热力图按周显示
    pub weekly: bool,
    pub hidden: bool,
    /// 额度用到这个百分比时提醒
    pub alert: u32,
    /// 额度、会话、Token 三个框拖出来的高度（DIP），0 = 跟着内容
    pub heights: [f32; 3],
    /// 磁贴底色的不透明度（20～100）
    pub opacity: u32,
}

/// 会话框默认最多 5 行，多了滚动
pub const DEFAULT_HEIGHTS: [f32; 3] = [0.0, 222.0, 0.0];

impl Default for Config {
    fn default() -> Config {
        Config { pos: [0; 3], topmost: false, en: None, cache_read: false, weekly: false, hidden: false, alert: 80, heights: DEFAULT_HEIGHTS, opacity: 100 }
    }
}

impl Config {
    pub fn parse(s: &str) -> Config {
        let mut c = Config::default();
        for l in s.trim_start_matches('\u{feff}').lines() {
            let Some((k, v)) = l.split_once('|') else { continue };
            let on = v == "1";
            match k {
                "pos" => {
                    let n: Vec<i32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                    if let [x, y, w] = n[..] {
                        c.pos = [x, y, w];
                    }
                }
                "topmost" => c.topmost = on,
                "lang" => c.en = Some(v == "en"),
                "cache_read" => c.cache_read = on,
                "weekly" => c.weekly = on,
                "hidden" => c.hidden = on,
                "alert" => c.alert = v.parse().unwrap_or(80),
                "opacity" => c.opacity = v.parse().unwrap_or(100),
                "heights" => {
                    let n: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                    if let [a, b, h] = n[..] {
                        c.heights = [a, b, h];
                    }
                }
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        let mut s = format!("pos|{},{},{}\ntopmost|{}\ncache_read|{}\nweekly|{}\nhidden|{}\nalert|{}\nheights|{},{},{}\nopacity|{}\n", self.pos[0], self.pos[1], self.pos[2], b(self.topmost), b(self.cache_read), b(self.weekly), b(self.hidden), self.alert, self.heights[0], self.heights[1], self.heights[2], self.opacity);
        if let Some(en) = self.en {
            s += if en { "lang|en\n" } else { "lang|zh\n" };
        }
        s
    }
}

static EN: AtomicBool = AtomicBool::new(false);

pub fn set_lang(en: Option<bool>) {
    // 主语言号在低 10 位，0x04 是中文
    EN.store(en.unwrap_or_else(|| (unsafe { GetUserDefaultUILanguage() } & 0x3FF) != 0x04), Relaxed);
}

pub fn en() -> bool {
    EN.load(Relaxed)
}

pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
    if self::en() { en } else { zh }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = Config { pos: [-10, 20, 400], topmost: true, en: Some(true), cache_read: true, weekly: true, hidden: false, alert: 90, heights: [0.0, 150.5, 300.0], opacity: 45 };
        assert_eq!(Config::parse(&c.to_text()), c);
        assert_eq!(Config::parse("\u{feff}pos|1,2\nbad\nlang|zh"), Config { en: Some(false), ..Default::default() });
    }
}
