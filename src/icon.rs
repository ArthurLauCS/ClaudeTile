//! 托盘图标：圆角方块里写 5 小时额度的百分比，底色表示状态：空闲陶土色、有会话在跑绿色、等你确认黄色、异常红色。
//! 用 D2D 画到 WIC 位图上，再拷进 32 位 DIB 做成图标，文字边缘有抗锯齿和透明度。

use windows::Win32::Foundation::TRUE;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::view::rgba;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tone {
    Idle,
    Busy,
    Waiting,
    Error,
}

/// 底色和文字色；黄底上白字看不清，用深色字。右下角提示卡片的状态方块也用这套颜色
pub fn colors(tone: Tone) -> (D2D1_COLOR_F, D2D1_COLOR_F) {
    let white = rgba(1.0, 1.0, 1.0, 1.0);
    match tone {
        Tone::Idle => (rgba(0.80, 0.42, 0.29, 1.0), white),
        Tone::Busy => (rgba(0.12, 0.62, 0.32, 1.0), white),
        Tone::Waiting => (rgba(0.98, 0.78, 0.18, 1.0), rgba(0.12, 0.10, 0.05, 1.0)),
        Tone::Error => (rgba(0.86, 0.22, 0.18, 1.0), white),
    }
}

/// pct 为 None 时（还没读到额度）只画方块
pub fn make(d2d: &ID2D1Factory1, dw: &IDWriteFactory, size: i32, pct: Option<f64>, tone: Tone) -> Option<HICON> {
    let px = pixels(d2d, dw, size, pct, tone)?;
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, Some(px.as_ptr() as _));
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: TRUE, hbmMask: mask, hbmColor: color, ..Default::default() };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// 图标像素（BGRA，预乘透明度，自上而下）
fn pixels(d2d: &ID2D1Factory1, dw: &IDWriteFactory, size: i32, pct: Option<f64>, tone: Tone) -> Option<Vec<u8>> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = wic.CreateBitmap(size as u32, size as u32, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, ..Default::default() };
        let rt = d2d.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
        let s = size as f32;
        let (c, ink) = colors(tone);
        let br = rt.CreateSolidColorBrush(&c, None).ok()?;
        rt.BeginDraw();
        rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
        let r = s * 0.22;
        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: 0.0, top: 0.0, right: s, bottom: s }, radiusX: r, radiusY: r }, &br);
        if let Some(p) = pct {
            // 100% 三位数放不下，写成满
            let t = if p >= 99.5 { "!!".to_string() } else { format!("{:.0}", p.max(0.0)) };
            let fs = if t.len() >= 2 { s * 0.62 } else { s * 0.72 };
            let f = dw.CreateTextFormat(w!("Segoe UI Variable Display"), None, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_CONDENSED, fs, w!("en-US")).ok()?;
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let w16: Vec<u16> = t.encode_utf16().collect();
            br.SetColor(&ink);
            rt.DrawText(&w16, &f, &D2D_RECT_F { left: -s, top: -s * 0.04, right: 2.0 * s, bottom: s }, &br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        rt.EndDraw(None, None).ok()?;
        let mut px = vec![0u8; (size * size * 4) as usize];
        bmp.CopyPixels(std::ptr::null(), (size * 4) as u32, &mut px).ok()?;
        Some(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四种状态各画一张 32px 的图，存成 BMP 放进临时目录（ctile-icon-0..3.bmp）供人工看，并确认画出了不透明的像素
    #[test]
    fn tones() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
            for (i, t) in [Tone::Idle, Tone::Busy, Tone::Waiting, Tone::Error].into_iter().enumerate() {
                let px = pixels(&d2d, &dw, 32, Some(36.0), t).unwrap();
                assert!(px.chunks(4).filter(|c| c[3] == 255).count() > 500);
                // 32 位 BMP：54 字节文件头和信息头，行自下而上
                let mut f = b"BM".to_vec();
                for v in [54u32 + 4096, 0, 54, 40, 32, 32] {
                    f.extend_from_slice(&v.to_le_bytes());
                }
                f.extend_from_slice(&1u16.to_le_bytes());
                f.extend_from_slice(&32u16.to_le_bytes());
                f.extend_from_slice(&[0; 24]);
                for row in px.chunks(128).rev() {
                    f.extend_from_slice(row);
                }
                std::fs::write(std::env::temp_dir().join(format!("ctile-icon-{i}.bmp")), f).unwrap();
            }
        }
    }
}
