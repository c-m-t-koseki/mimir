//! The input mode button: あ / A in the taskbar's input indicator, and the
//! menu a click on it opens (設定… starts `mimir-settings.exe`).

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateBitmap,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FW_NORMAL, HBITMAP, HGDIOBJ,
    OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Ole::CONNECT_E_CANNOTCONNECT;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::TextServices::{
    GUID_LBI_INPUTMODE, ITfLangBarItem, ITfLangBarItem_Impl, ITfLangBarItemButton,
    ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfMenu, ITfSource, ITfSource_Impl,
    TF_LANGBARITEMINFO, TF_LBI_ICON, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY,
    TF_LBI_TEXT, TfLBIClick,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow,
    GetSystemMetrics, HICON, ICONINFO, MF_STRING, PostMessageW, SM_CXSMICON, SetForegroundWindow,
    TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    WINDOW_EX_STYLE, WM_NULL, WS_POPUP,
};
use windows::core::{BOOL, BSTR, GUID, IUnknown, Interface, Ref, Result, implement, w};

use super::{CLSID_MIMIR, NAME};

const MENU_SETTINGS: u32 = 1;

/// What the button shows, and whom to tell when it changes.
#[derive(Default)]
pub struct ModeState {
    ascii: bool,
    sink: Option<ITfLangBarItemSink>,
}

/// The engine's mode as `KeyResult::mode` reports it ("kana" / "ascii"):
/// redraw the button when it changes.
pub fn set_mode(state: &RefCell<ModeState>, mode: &str) {
    let ascii = mode == "ascii";
    let sink = {
        let mut state = state.borrow_mut();
        if mode.is_empty() || state.ascii == ascii {
            return;
        }
        state.ascii = ascii;
        state.sink.clone()
    };
    // Outside the borrow: the sink asks for the new icon right away.
    if let Some(sink) = sink {
        unsafe {
            let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT);
        }
    }
}

#[implement(ITfLangBarItemButton, ITfLangBarItem, ITfSource)]
pub struct ModeButton {
    state: Rc<RefCell<ModeState>>,
}

impl ModeButton {
    pub fn new(state: Rc<RefCell<ModeState>>) -> Self {
        Self { state }
    }
}

impl ModeButton_Impl {
    fn label(&self) -> &'static str {
        if self.state.borrow().ascii {
            "A"
        } else {
            "あ"
        }
    }
}

impl ITfLangBarItem_Impl for ModeButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        let mut description = [0u16; 32];
        for (d, c) in description.iter_mut().zip(NAME.encode_utf16()) {
            *d = c;
        }
        unsafe {
            *info = TF_LANGBARITEMINFO {
                clsidService: CLSID_MIMIR,
                guidItem: GUID_LBI_INPUTMODE,
                dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
                ulSort: 0,
                szDescription: description,
            };
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }

    fn Show(&self, _show: BOOL) -> Result<()> {
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::from(NAME))
    }
}

impl ITfLangBarItemButton_Impl for ModeButton_Impl {
    /// The taskbar's input indicator only reports the click (it never asks
    /// InitMenu for the menu), so the menu is shown here, left click or right.
    /// ponytail: a left click doesn't switch the mode yet; that needs a
    /// document to apply the engine's answer to.
    fn OnClick(&self, _click: TfLBIClick, pt: &POINT, _area: *const RECT) -> Result<()> {
        if unsafe { popup_menu(*pt) }? == MENU_SETTINGS {
            open_settings();
        }
        Ok(())
    }

    /// The floating language bar's menu (Windows 7's, or the one still
    /// shown by the classic language bar setting).
    fn InitMenu(&self, menu: Ref<ITfMenu>) -> Result<()> {
        let label: Vec<u16> = "設定…".encode_utf16().collect();
        unsafe {
            menu.ok()?.AddMenuItem(
                MENU_SETTINGS,
                0,
                HBITMAP::default(),
                HBITMAP::default(),
                &label,
                std::ptr::null_mut(),
            )
        }
    }

    fn OnMenuSelect(&self, id: u32) -> Result<()> {
        if id == MENU_SETTINGS {
            open_settings();
        }
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        unsafe { text_icon(self.label()) }
    }

    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.label()))
    }
}

impl ITfSource_Impl for ModeButton_Impl {
    fn AdviseSink(&self, riid: *const GUID, sink: Ref<IUnknown>) -> Result<u32> {
        if unsafe { *riid } != ITfLangBarItemSink::IID {
            return Err(CONNECT_E_CANNOTCONNECT.into());
        }
        self.state.borrow_mut().sink = Some(sink.ok()?.cast()?);
        Ok(1)
    }

    fn UnadviseSink(&self, _cookie: u32) -> Result<()> {
        self.state.borrow_mut().sink = None;
        Ok(())
    }
}

/// `mimir-settings.exe`, installed next to this DLL.
fn open_settings() {
    let dll = super::module_path();
    let settings = Path::new(&dll).with_file_name("mimir-settings.exe");
    // Nothing to show a failure in from here: the menu is gone.
    let _ = std::process::Command::new(settings).spawn();
}

/// The button's menu at `pt` (screen coordinates, above it: the taskbar is
/// at the bottom); the chosen item's id, 0 for none.
unsafe fn popup_menu(pt: POINT) -> Result<u32> {
    unsafe {
        let menu = CreatePopupMenu()?;
        AppendMenuW(menu, MF_STRING, MENU_SETTINGS as usize, w!("設定…"))?;
        // A menu closes on a click elsewhere only when a window of this
        // thread owning it is in the foreground.
        let owner = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            None,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )?;
        let _ = SetForegroundWindow(owner);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let chosen = TrackPopupMenuEx(menu, flags.0, pt.x, pt.y, owner, None);
        // Lets the next click after a dismissed menu reach its target.
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyWindow(owner);
        let _ = DestroyMenu(menu);
        Ok(chosen.0 as u32)
    }
}

/// Whether the taskbar is light (Settings → Personalization → Colors).
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut len = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut len),
        )
    };
    result == ERROR_SUCCESS && value != 0
}

/// `text` drawn as a small icon, in the taskbar's text color. The caller
/// (TSF) destroys it.
unsafe fn text_icon(text: &str) -> Result<HICON> {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let font = CreateFontW(
            -size * 7 / 8,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Yu Gothic UI"),
        );
        let old_bitmap = SelectObject(dc, HGDIOBJ(color.0));
        let old_font = SelectObject(dc, HGDIOBJ(font.0));
        // White on the zeroed (black) bitmap: each pixel's red is how much
        // of it the glyph covers, which becomes its alpha below.
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00ff_ffff));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        DrawTextW(
            dc,
            &mut wide,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);

        let rgb: u32 = if light_taskbar() {
            0x00_00_00
        } else {
            0xff_ff_ff
        };
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize);
        for pixel in pixels {
            let alpha = *pixel & 0xff;
            *pixel = (alpha << 24) | rgb;
        }
        // All clear: the color bitmap's alpha is what shows.
        let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteDC(dc);
        icon
    }
}
