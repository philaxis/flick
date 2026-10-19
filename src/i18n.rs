//! Small shared string selector, using the same Windows UI language as the guide.
pub fn language(id: u16) -> usize {
    match id & 0x3ff { 0x12 => 1, 0x04 => 2, _ => 0 }
}
pub fn text<'a>(en: &'a str, ko: &'a str, zh: &'a str) -> &'a str {
    [en, ko, zh][language(unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() })]
}
#[cfg(test)]
mod tests {
    #[test]
    fn ui_language_uses_primary_language_and_falls_back_to_english() {
        assert_eq!([0x0412, 0x0804, 0x0404, 0x0409, 0x0411].map(super::language), [1, 2, 2, 0, 0]);
    }
}
