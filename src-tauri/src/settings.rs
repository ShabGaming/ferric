use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub close_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let temporary = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let mut file = fs::File::create(&temporary)?;
        use io::Write;
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_keep_close_to_tray_enabled() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.close_to_tray);
    }

    #[test]
    fn preferences_round_trip_and_invalid_data_is_preserved() {
        let directory =
            std::env::temp_dir().join(format!("ferric-settings-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        assert!(Settings::load(&path).unwrap().close_to_tray);
        Settings {
            close_to_tray: false,
        }
        .save(&path)
        .unwrap();
        assert!(!Settings::load(&path).unwrap().close_to_tray);
        Settings::default().save(&path).unwrap();
        assert!(Settings::load(&path).unwrap().close_to_tray);
        fs::write(&path, "invalid").unwrap();
        assert!(Settings::load(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid");
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
