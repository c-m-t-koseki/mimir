//! Windows virtual-key codes to the XKB keysyms the engine speaks.
//!
//! Printable keys are resolved through the active keyboard layout
//! (`ToUnicodeEx`, in the TSF glue) and reach [`keysym_for_char`];
//! everything else goes through [`keysym_for_vk`].

/// XKB `Eisu_toggle`: the engine's switch to ASCII mode (英数).
pub const EISU_TOGGLE: u32 = 0xff2f;
/// XKB `Super_R`: the engine's return to kana (かな).
pub const SUPER_R: u32 = 0xffec;

/// The keysym for a non-printable virtual key, `None` for keys the IME
/// leaves alone (modifiers, media keys, …).
pub fn keysym_for_vk(vk: u16) -> Option<u32> {
    Some(match vk {
        0x08 => 0xff08,                               // VK_BACK
        0x09 => 0xff09,                               // VK_TAB
        0x0d => 0xff0d,                               // VK_RETURN
        0x1b => 0xff1b,                               // VK_ESCAPE
        0x20 => 0x0020,                               // VK_SPACE
        0x21 => 0xff55,                               // VK_PRIOR
        0x22 => 0xff56,                               // VK_NEXT
        0x23 => 0xff57,                               // VK_END
        0x24 => 0xff50,                               // VK_HOME
        0x25 => 0xff51,                               // VK_LEFT
        0x26 => 0xff52,                               // VK_UP
        0x27 => 0xff53,                               // VK_RIGHT
        0x28 => 0xff54,                               // VK_DOWN
        0x2e => 0xffff,                               // VK_DELETE
        0x70..=0x7b => 0xffbe + u32::from(vk - 0x70), // VK_F1..VK_F12
        // JIS keyboard: 無変換 and 英数 go to ASCII; 変換, カタカナ/ひらがな
        // and かな come back to kana.
        0x1d | 0xf0 => EISU_TOGGLE, // VK_NONCONVERT, VK_DBE_ALPHANUMERIC
        0x1c | 0x15 | 0xf1 | 0xf2 => SUPER_R, // VK_CONVERT, VK_KANA, VK_DBE_KATAKANA, VK_DBE_HIRAGANA
        _ => return None,
    })
}

/// The keysym for a printable character: Latin-1 maps to itself, as in
/// XKB; anything beyond gets the Unicode keysym range.
pub fn keysym_for_char(ch: char) -> u32 {
    match ch as u32 {
        c @ 0x20..=0x7e | c @ 0xa0..=0xff => c,
        c => 0x0100_0000 | c,
    }
}

/// Keys that must never reach the engine as a key of their own.
pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x14 | 0x5b | 0x5c | 0xa0..=0xa5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_keys() {
        assert_eq!(keysym_for_vk(0x08), Some(0xff08));
        assert_eq!(keysym_for_vk(0x0d), Some(0xff0d));
        assert_eq!(keysym_for_vk(0x7b), Some(0xffc9)); // F12
        assert_eq!(keysym_for_vk(0x1d), Some(EISU_TOGGLE));
        assert_eq!(keysym_for_vk(0xf2), Some(SUPER_R));
        assert_eq!(keysym_for_vk(0x41), None); // letters go through the layout
    }

    #[test]
    fn printable_chars() {
        assert_eq!(keysym_for_char('a'), 0x61);
        assert_eq!(keysym_for_char('A'), 0x41);
        assert_eq!(keysym_for_char(','), 0x2c);
        assert_eq!(keysym_for_char('¥'), 0xa5);
        assert_eq!(keysym_for_char('あ'), 0x0100_3042);
    }
}
