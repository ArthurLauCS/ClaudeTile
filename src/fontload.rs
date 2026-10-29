//! Fonts shipped with the app: the `fonts` folder next to the exe (`assets/fonts` in a dev run), fetched by
//! scripts/fonts.ps1. Built as one DirectWrite font collection (GDI FR_PRIVATE fonts are invisible to
//! DirectWrite). Every step that fails yields None / the system face, never an error.

use std::path::PathBuf;

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;

use crate::theme::{FONT_CJK, FONT_FILES};

pub struct Bundled {
    /// Sarasa Mono SC + LXGW WenKai Mono; None when no font file was found or loading failed
    pub collection: Option<IDWriteFontCollection>,
    /// CJK ranges go to YaHei UI first, then the system chain: no tofu boxes for glyphs a face lacks
    pub fallback: Option<IDWriteFontFallback>,
}

/// Next to the exe first, then the dev layout target\<profile>\claudetile.exe -> ..\..\assets\fonts
fn font_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("fonts"), dir.join("..").join("..").join("assets").join("fonts")].into_iter().find(|d| FONT_FILES.iter().any(|f| d.join(f).is_file()))
}

fn collection(dw3: &IDWriteFactory3, dir: &std::path::Path) -> Option<IDWriteFontCollection> {
    unsafe {
        let builder = dw3.CreateFontSetBuilder().ok()?;
        let mut any = false;
        for name in FONT_FILES {
            let path = dir.join(name);
            if !path.is_file() {
                continue;
            }
            let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
            if let Ok(face) = dw3.CreateFontFaceReference2(PCWSTR(wide.as_ptr()), None, 0, DWRITE_FONT_SIMULATIONS_NONE) {
                any |= builder.AddFontFaceReference2(&face).is_ok();
            }
        }
        if !any {
            return None;
        }
        let set = builder.CreateFontSet().ok()?;
        dw3.CreateFontCollectionFromFontSet(&set).ok()?.cast().ok()
    }
}

fn fallback(dw: &IDWriteFactory) -> Option<IDWriteFontFallback> {
    unsafe {
        let dw2: IDWriteFactory2 = dw.cast().ok()?;
        let b = dw2.CreateFontFallbackBuilder().ok()?;
        // Han, radicals, CJK punctuation, full-width forms, compatibility, extension B and later
        let ranges = [(0x2E80, 0x2FDF), (0x3000, 0x303F), (0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF), (0xFF00, 0xFFEF), (0x20000, 0x2FA1F)]
            .map(|(first, last)| DWRITE_UNICODE_RANGE { first, last });
        let name: Vec<u16> = FONT_CJK.encode_utf16().chain([0]).collect();
        b.AddMapping(&ranges, &[name.as_ptr()], None, None, PCWSTR::null(), 1.0).ok()?;
        if let Ok(sys) = dw2.GetSystemFontFallback() {
            let _ = b.AddMappings(&sys);
        }
        b.CreateFontFallback().ok()
    }
}

impl Bundled {
    pub fn load(dw: &IDWriteFactory) -> Bundled {
        let collection = font_dir().and_then(|dir| collection(&dw.cast::<IDWriteFactory3>().ok()?, &dir));
        Bundled { collection, fallback: fallback(dw) }
    }

    /// True when the bundled collection has this family (a single missing file leaves the others usable)
    pub fn has(&self, family: &str) -> bool {
        let Some(c) = &self.collection else { return false };
        let name: Vec<u16> = family.encode_utf16().chain([0]).collect();
        let (mut idx, mut found) = (0u32, windows::core::BOOL(0));
        unsafe { c.FindFamilyName(PCWSTR(name.as_ptr()), &mut idx, &mut found).is_ok() && found.as_bool() }
    }
}
