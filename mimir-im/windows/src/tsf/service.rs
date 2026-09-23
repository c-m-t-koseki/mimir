//! The text service: keys to the engine, its actions into the document.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor,
    ITfTextInputProcessor_Impl, ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl,
    ITfThreadMgr, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START, TF_ES_READWRITE, TF_ES_SYNC,
    TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::core::{BOOL, GUID, Interface, Ref, Result, implement};
use windows_core::IUnknownImpl;

use crate::client::{Action, EngineClient, KeyResult, Modifiers};
use crate::keymap::{is_modifier_vk, keysym_for_char, keysym_for_vk};

/// What the service and its edit sessions share.
#[derive(Default)]
struct State {
    thread_mgr: Option<ITfThreadMgr>,
    client_id: u32,
    engine: Option<EngineClient>,
    /// The composition holding the engine's preedit.
    composition: Option<ITfComposition>,
    /// `OnTestKeyDown` asks the engine (whose answer has effects, so it
    /// can't be asked twice); `OnKeyDown` applies the answer.
    pending: Option<(u16, KeyResult)>,
}

#[implement(
    ITfTextInputProcessorEx,
    ITfTextInputProcessor,
    ITfKeyEventSink,
    ITfCompositionSink
)]
pub struct TextService {
    state: Rc<RefCell<State>>,
}

impl TextService {
    pub fn new() -> Self {
        Self {
            state: Rc::default(),
        }
    }
}

/// `mimir-imserver.exe`, installed next to this DLL.
fn server_path() -> std::path::PathBuf {
    let dll = super::module_path();
    Path::new(&dll).with_file_name("mimir-imserver.exe")
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, thread_mgr: Ref<ITfThreadMgr>, client_id: u32) -> Result<()> {
        let thread_mgr = thread_mgr.ok()?.clone();
        let sink: ITfKeyEventSink = self.to_interface();
        unsafe {
            thread_mgr
                .cast::<ITfKeystrokeMgr>()?
                .AdviseKeyEventSink(client_id, &sink, true)?;
        }
        let mut state = self.state.borrow_mut();
        state.thread_mgr = Some(thread_mgr);
        state.client_id = client_id;
        // ponytail: one server per application process; a shared server
        // (one model load for every app) is the next step, see docs/windows.md.
        state.engine = EngineClient::spawn(&server_path()).ok();
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if let Some(thread_mgr) = state.thread_mgr.take() {
            unsafe {
                let _ = thread_mgr
                    .cast::<ITfKeystrokeMgr>()
                    .and_then(|keys| keys.UnadviseKeyEventSink(state.client_id));
            }
        }
        state.composition = None;
        state.pending = None;
        state.engine = None;
        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(&self, thread_mgr: Ref<ITfThreadMgr>, client_id: u32, _flags: u32) -> Result<()> {
        ITfTextInputProcessor_Impl::Activate(self, thread_mgr, client_id)
    }
}

/// The key as the engine sees it: a keysym and the modifiers held.
fn translate(vk: u16, lparam: LPARAM) -> Option<(u32, Modifiers)> {
    if is_modifier_vk(vk) {
        return None;
    }
    let mut keys = [0u8; 256];
    unsafe { GetKeyboardState(&mut keys).ok()? };
    let held = |vk: u16| keys[usize::from(vk)] & 0x80 != 0;
    let modifiers = Modifiers {
        shift: held(VK_SHIFT.0),
        control: held(VK_CONTROL.0),
        alt: held(VK_MENU.0),
    };
    if let Some(keysym) = keysym_for_vk(vk) {
        return Some((keysym, modifiers));
    }
    // Printable: the character the layout gives without Ctrl/Alt, so
    // Ctrl+N arrives as `n` with control held, as the engine expects.
    for vk in [
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
    ] {
        keys[usize::from(vk.0)] = 0;
    }
    let scan = ((lparam.0 >> 16) & 0xff) as u32;
    let mut buf = [0u16; 4];
    // Flag 4: leave the keyboard state (dead keys) alone.
    let n = unsafe {
        ToUnicodeEx(
            u32::from(vk),
            scan,
            &keys,
            &mut buf,
            4,
            Some(GetKeyboardLayout(0)),
        )
    };
    let ch = char::decode_utf16(
        buf[..usize::try_from(n).ok().filter(|&n| n == 1)?]
            .iter()
            .copied(),
    )
    .next()?
    .ok()?;
    Some((keysym_for_char(ch), modifiers))
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: Ref<ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = wparam.0 as u16;
        let mut state = self.state.borrow_mut();
        state.pending = None;
        let Some((keysym, modifiers)) = translate(vk, lparam) else {
            return Ok(false.into());
        };
        let Some(result) = state
            .engine
            .as_mut()
            .and_then(|e| e.process_key(keysym, modifiers))
        else {
            return Ok(false.into());
        };
        let consumed = result.consumed;
        state.pending = Some((vk, result));
        Ok(consumed.into())
    }

    fn OnKeyDown(&self, context: Ref<ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let vk = wparam.0 as u16;
        let pending = self.state.borrow_mut().pending.take();
        let result = match pending {
            Some((pending_vk, result)) if pending_vk == vk => Some(result),
            // Not tested first (TSF may skip the test): ask now.
            _ => {
                let mut state = self.state.borrow_mut();
                translate(vk, lparam).and_then(|(keysym, modifiers)| {
                    state.engine.as_mut()?.process_key(keysym, modifiers)
                })
            }
        };
        let Some(result) = result.filter(|r| r.consumed) else {
            return Ok(false.into());
        };
        let context = context.ok()?;
        self.apply(context, result.actions)?;
        Ok(true.into())
    }

    fn OnTestKeyUp(&self, _: Ref<ITfContext>, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnKeyUp(&self, _: Ref<ITfContext>, _: WPARAM, _: LPARAM) -> Result<BOOL> {
        Ok(false.into())
    }

    fn OnPreservedKey(&self, _: Ref<ITfContext>, _: *const GUID) -> Result<BOOL> {
        Ok(false.into())
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    /// The application ended the composition (a click elsewhere): the
    /// preedit stays as typed, and the engine lets go of it.
    fn OnCompositionTerminated(&self, _ec: u32, _composition: Ref<ITfComposition>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.composition = None;
        if let Some(engine) = state.engine.as_mut() {
            engine.commit();
        }
        Ok(())
    }
}

impl TextService_Impl {
    /// Apply the engine's actions to the document in a synchronous edit
    /// session (the only way to write to it).
    fn apply(&self, context: &ITfContext, actions: Vec<Action>) -> Result<()> {
        let session: ITfEditSession = EditSession {
            state: self.state.clone(),
            context: context.clone(),
            sink: self.to_interface(),
            actions: RefCell::new(actions),
        }
        .into();
        let client_id = self.state.borrow().client_id;
        unsafe {
            context
                .RequestEditSession(client_id, &session, TF_ES_SYNC | TF_ES_READWRITE)?
                .ok()
        }
    }
}

#[implement(ITfEditSession)]
struct EditSession {
    state: Rc<RefCell<State>>,
    context: ITfContext,
    sink: ITfCompositionSink,
    actions: RefCell<Vec<Action>>,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        // ponytail: candidate window and aux text not drawn yet; Ctrl+N/P
        // still cycle the candidate shown in the preedit.
        for action in self.actions.take() {
            match action {
                Action::UpdatePreedit { text, caret, .. } => {
                    self.update_preedit(ec, &text, caret)?
                }
                Action::Commit { text } => self.commit(ec, &text)?,
                _ => {}
            }
        }
        Ok(())
    }
}

impl EditSession_Impl {
    fn update_preedit(&self, ec: u32, text: &str, caret: usize) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if text.is_empty() {
            if let Some(composition) = state.composition.take() {
                unsafe {
                    composition.GetRange()?.SetText(ec, 0, &[])?;
                    composition.EndComposition(ec)?;
                }
            }
            return Ok(());
        }
        let composition = match state.composition.clone() {
            Some(composition) => composition,
            None => {
                let composition = self.start_composition(ec)?;
                state.composition = Some(composition.clone());
                composition
            }
        };
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let caret16: usize = text.chars().take(caret).map(char::len_utf16).sum();
        unsafe {
            let range = composition.GetRange()?;
            range.SetText(ec, 0, &utf16)?;
            // Caret: collapse to the start, then move right.
            let caret_range = range.Clone()?;
            caret_range.Collapse(ec, TF_ANCHOR_START)?;
            let mut moved = 0;
            caret_range.ShiftEnd(ec, caret16 as i32, &mut moved, std::ptr::null())?;
            caret_range.Collapse(ec, TF_ANCHOR_END)?;
            self.select(ec, &caret_range)?;
        }
        Ok(())
    }

    fn commit(&self, ec: u32, text: &str) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let composition = self.state.borrow_mut().composition.take();
        unsafe {
            let range = match &composition {
                Some(composition) => {
                    let range = composition.GetRange()?;
                    range.SetText(ec, 0, &utf16)?;
                    range
                }
                None => self
                    .context
                    .cast::<ITfInsertAtSelection>()?
                    .InsertTextAtSelection(ec, Default::default(), &utf16)?,
            };
            range.Collapse(ec, TF_ANCHOR_END)?;
            self.select(ec, &range)?;
            if let Some(composition) = composition {
                composition.EndComposition(ec)?;
            }
        }
        Ok(())
    }

    /// A composition at the selection.
    fn start_composition(&self, ec: u32) -> Result<ITfComposition> {
        unsafe {
            let range = self
                .context
                .cast::<ITfInsertAtSelection>()?
                .InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;
            self.context
                .cast::<ITfContextComposition>()?
                .StartComposition(ec, &range, &self.sink)
        }
    }

    unsafe fn select(&self, ec: u32, range: &ITfRange) -> Result<()> {
        let selection = TF_SELECTION {
            range: std::mem::ManuallyDrop::new(Some(range.clone())),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };
        let result = unsafe {
            self.context
                .SetSelection(ec, std::slice::from_ref(&selection))
        };
        drop(std::mem::ManuallyDrop::into_inner(selection.range));
        result
    }
}
