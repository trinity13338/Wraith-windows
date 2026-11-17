//! Russian or English, picked from the Windows display language.

use std::sync::OnceLock;

/// True unless Windows is in Russian, Ukrainian, Belarusian or Kazakh.
pub fn en() -> bool {
    static EN: OnceLock<bool> = OnceLock::new();
    *EN.get_or_init(|| {
        if let Ok(l) = std::env::var("WRAITH_LANG") { return !l.starts_with("ru"); }
        let lang = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff;
        !matches!(lang, 0x19 | 0x22 | 0x23 | 0x3f)
    })
}
pub fn t(ru: &'static str, en_text: &'static str) -> &'static str { if en() { en_text } else { ru } }

#[macro_export]
macro_rules! tr { ($ru:expr, $en:expr) => { $crate::i18n::t($ru, $en) }; }
#[macro_export]
macro_rules! trf { ($ru:literal, $en:literal $(, $($a:tt)*)?) => { if $crate::i18n::en() { format!($en $(, $($a)*)?) } else { format!($ru $(, $($a)*)?) } }; }
