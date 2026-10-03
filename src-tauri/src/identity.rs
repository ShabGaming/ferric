use std::{
    fs,
    path::{Path, PathBuf},
};
use webview2_com::take_pwstr;
use windows::{
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IPersistFile, STGM_READ, StructuredStorage::PROPVARIANT,
            },
            Variant::VT_LPWSTR,
        },
        UI::Shell::{
            FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, PropertiesSystem::IPropertyStore,
            SHGetKnownFolderPath, SHStrDupW, SLGP_RAWPATH, SetCurrentProcessExplicitAppUserModelID,
            ShellLink,
        },
    },
    core::{HSTRING, Interface},
};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
};

pub fn set_process_id(identifier: &str) -> windows::core::Result<()> {
    // I set the identity before creating windows or the WebView2 environment.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(identifier)) }
}

pub fn register(
    identifier: &str,
    name: &str,
    directory: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let branding = directory.join("branding");
    fs::create_dir_all(&branding)?;
    let icon = branding.join("icon.png");
    let image = include_bytes!("../icons/icon.png");
    if fs::read(&icon).as_deref().ok() != Some(image.as_slice()) {
        fs::write(&icon, image)?;
    }
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(
        format!(r"Software\Classes\AppUserModelId\{identifier}"),
        KEY_READ | KEY_SET_VALUE,
    )?;
    for (property, value) in [
        ("DisplayName", name.to_owned()),
        ("IconUri", icon.to_string_lossy().into_owned()),
    ] {
        if key.get_value::<String, _>(property).ok().as_ref() != Some(&value) {
            key.set_value(property, &value)?;
        }
    }
    register_shortcut(identifier, name)?;
    Ok(())
}

fn register_shortcut(identifier: &str, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        initialized.ok()?;
    }
    let _apartment = ComApartment(initialized.is_ok());
    // I give the shell a shortcut with the same identity as my native media session.
    let programs =
        take_pwstr(unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)? });
    let folder = PathBuf::from(programs).join("Ferric");
    fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{name}.lnk"));
    let filename = HSTRING::from(path.as_os_str());
    let executable = std::env::current_exe()?;
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    let file: IPersistFile = link.cast()?;
    let properties: IPropertyStore = link.cast()?;
    if path.exists() && unsafe { file.Load(&filename, STGM_READ) }.is_ok() {
        let mut target = vec![0u16; 32_768];
        let target_matches =
            unsafe { link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32) }
                .is_ok()
                && Path::new(&String::from_utf16_lossy(
                    &target[..target.iter().position(|c| *c == 0).unwrap_or(target.len())],
                )) == executable;
        let identity_matches = unsafe { properties.GetValue(&PKEY_AppUserModel_ID) }
            .is_ok_and(|value| value.to_string() == identifier);
        if target_matches && identity_matches {
            return Ok(());
        }
    }
    let mut identity = PROPVARIANT::default();
    unsafe {
        // I allocate the string with COM's allocator; PROPVARIANT releases it on drop.
        let value = &mut *identity.Anonymous.Anonymous;
        value.vt = VT_LPWSTR;
        value.Anonymous.pwszVal = SHStrDupW(&HSTRING::from(identifier))?;
        link.SetPath(&HSTRING::from(executable.as_os_str()))?;
        link.SetDescription(&HSTRING::from(name))?;
        link.SetIconLocation(&HSTRING::from(executable.as_os_str()), 0)?;
        if let Some(parent) = executable.parent() {
            link.SetWorkingDirectory(&HSTRING::from(parent.as_os_str()))?;
        }
        properties.SetValue(&PKEY_AppUserModel_ID, &identity)?;
        properties.Commit()?;
        file.Save(&filename, true)?;
    }
    Ok(())
}

struct ComApartment(bool);

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
