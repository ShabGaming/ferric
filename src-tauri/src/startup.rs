use std::{io, path::Path};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Ferric";

pub fn enabled() -> io::Result<bool> {
    match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ) {
        Ok(key) => match key.get_value::<String, _>(VALUE) {
            Ok(value) => Ok(!value.is_empty()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn command(executable: &Path) -> io::Result<String> {
    let path = executable
        .to_str()
        .ok_or_else(|| io::Error::other("The app path is not valid UTF-8"))?;
    if path.contains('"') {
        return Err(io::Error::other("The app path contains a quote"));
    }
    let command = format!("\"{path}\" --startup");
    if command.encode_utf16().count() > 260 {
        return Err(io::Error::other(
            "Move YouTube Music to a shorter path before enabling startup",
        ));
    }
    Ok(command)
}

pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let (key, _) =
        RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)?;
    if enabled {
        key.set_value(VALUE, &command(&std::env::current_exe()?)?)
    } else {
        match key.delete_value(VALUE) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

pub fn refresh_registration() -> io::Result<()> {
    if enabled()? {
        set_enabled(true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_quotes_the_executable_and_starts_in_the_tray() {
        assert_eq!(
            command(Path::new(r"C:\My Apps\ferric.exe")).unwrap(),
            r#""C:\My Apps\ferric.exe" --startup"#
        );
        assert!(command(Path::new("bad\"path.exe")).is_err());
        assert!(command(Path::new(&"a".repeat(261))).is_err());
    }
}
