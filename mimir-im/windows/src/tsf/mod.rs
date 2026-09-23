//! The COM DLL: exports, class factory, and TSF registration.

mod service;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_NOINTERFACE, HINSTANCE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    IClassFactory, IClassFactory_Impl,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT, ITfCategoryMgr, ITfInputProcessorProfileMgr,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR, Ref, Result, implement};

use service::TextService;

/// The text service's COM class.
pub const CLSID_MIMIR: GUID = GUID::from_u128(0xc18202f0_ddf2_408e_8f95_1adcad04bcaf);
/// Its one language profile: Japanese.
const PROFILE_MIMIR: GUID = GUID::from_u128(0x03791a31_fb5c_4756_abed_9c1f2aee7874);
const LANGID_JAPANESE: u16 = 0x0411;
const NAME: &str = "Mimir";

/// This DLL's module handle, for its own path.
static MODULE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// This DLL's path.
fn module_path() -> String {
    let mut buf = [0u16; 1024];
    let module = HMODULE(MODULE.load(Ordering::Relaxed));
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..len])
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0, Ordering::Relaxed);
    }
    BOOL::from(true)
}

#[implement(IClassFactory)]
struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(E_NOINTERFACE.into());
        }
        let service: IUnknown = TextService::new().into();
        unsafe { service.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if unsafe { *clsid } != CLSID_MIMIR {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = ClassFactory.into();
    unsafe { factory.query(iid, object) }
}

/// ponytail: never unloads; count live objects if that ever matters.
#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    match register() {
        Ok(()) => S_OK,
        Err(e) => {
            let _ = unregister();
            if e.code().is_err() { e.code() } else { E_FAIL }
        }
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    match unregister() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

fn clsid_key() -> String {
    format!("CLSID\\{{{CLSID_MIMIR:?}}}")
}

/// `HKCR\CLSID\{clsid}\InprocServer32` → this DLL, then the TSF profile
/// (Japanese keyboard) and categories.
fn register() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        set_reg_value(&clsid_key(), None, NAME)?;
        let inproc = format!("{}\\InprocServer32", clsid_key());
        set_reg_value(&inproc, None, &module_path())?;
        set_reg_value(&inproc, Some("ThreadingModel"), "Apartment")?;

        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let name: Vec<u16> = NAME.encode_utf16().collect();
        profiles.RegisterProfile(
            &CLSID_MIMIR,
            LANGID_JAPANESE,
            &PROFILE_MIMIR,
            &name,
            &[],
            0,
            HKL::default(),
            0,
            true,
            0,
        )?;

        let categories: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for category in [GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT] {
            categories.RegisterCategory(&CLSID_MIMIR, &category, &CLSID_MIMIR)?;
        }
    }
    Ok(())
}

fn unregister() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Ok(categories) =
            CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
        {
            for category in [GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT] {
                let _ = categories.UnregisterCategory(&CLSID_MIMIR, &category, &CLSID_MIMIR);
            }
        }
        if let Ok(profiles) = CoCreateInstance::<_, ITfInputProcessorProfileMgr>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = profiles.UnregisterProfile(&CLSID_MIMIR, LANGID_JAPANESE, &PROFILE_MIMIR, 0);
        }
        let key = wide(&clsid_key());
        let _ = RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(key.as_ptr()));
    }
    Ok(())
}

/// `HKCR\{path}` `[name] = value` (the default value for `None`).
unsafe fn set_reg_value(path: &str, name: Option<&str>, value: &str) -> Result<()> {
    let path = wide(path);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let name = name.map(wide);
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let result = RegSetValueExW(
            key,
            name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            None,
            REG_SZ,
            Some(&data),
        );
        let _ = RegCloseKey(key);
        result.ok()
    }
}
