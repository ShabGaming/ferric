use crate::{AppState, playback};
use tauri::{Manager, WebviewWindow};
use webview2_com::{WebMessageReceivedEventHandler, take_pwstr};
use windows::{
    Foundation::{TypedEventHandler, Uri},
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    },
    Storage::Streams::RandomAccessStreamReference,
    Win32::System::WinRT::ISystemMediaTransportControlsInterop,
    core::{HSTRING, PWSTR, factory},
};

fn artwork_uri(value: &str) -> Option<Uri> {
    if !playback::valid_artwork(value) {
        return None;
    }
    Uri::CreateUri(&HSTRING::from(value)).ok()
}

pub struct MediaSession {
    controls: SystemMediaTransportControls,
    button_token: i64,
}

impl MediaSession {
    pub fn clear(&self) {
        let _ = self.controls.SetIsEnabled(false);
        let _ = self.controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        let _ = self.controls.SetIsEnabled(false);
        let _ = self.controls.RemoveButtonPressed(self.button_token);
    }
}

pub fn attach(window: &WebviewWindow) -> Result<MediaSession, Box<dyn std::error::Error>> {
    let session = create_controls(window);
    // Install the metadata bridge even when Windows media controls are unavailable.
    let native = session
        .as_ref()
        .ok()
        .map(|session| session.controls.clone());
    attach_bridge(window, native)?;
    session
}

fn create_controls(window: &WebviewWindow) -> Result<MediaSession, Box<dyn std::error::Error>> {
    let interop: ISystemMediaTransportControlsInterop =
        factory::<SystemMediaTransportControls, _>()?;
    // I bind the controls to my own top-level window, keeping WebView2 out of the app identity.
    let controls: SystemMediaTransportControls = unsafe { interop.GetForWindow(window.hwnd()?)? };
    controls.SetIsEnabled(false)?;
    controls.SetIsPlayEnabled(true)?;
    controls.SetIsPauseEnabled(true)?;
    controls.SetIsNextEnabled(true)?;
    controls.SetIsPreviousEnabled(true)?;
    let player = window.clone();
    let handler = TypedEventHandler::<
        SystemMediaTransportControls,
        SystemMediaTransportControlsButtonPressedEventArgs,
    >::new(move |_, event| {
        let Some(event) = event.as_ref() else {
            return Ok(());
        };
        let action = match event.Button()? {
            SystemMediaTransportControlsButton::Play => "play",
            SystemMediaTransportControlsButton::Pause => "pause",
            SystemMediaTransportControlsButton::Next => "nexttrack",
            SystemMediaTransportControlsButton::Previous => "previoustrack",
            _ => return Ok(()),
        };
        let _ = player.eval(format!("window.__ferricMediaAction?.('{action}')"));
        Ok(())
    });
    let button_token = controls.ButtonPressed(&handler)?;
    Ok(MediaSession {
        controls,
        button_token,
    })
}

fn attach_bridge(
    window: &WebviewWindow,
    native: Option<SystemMediaTransportControls>,
) -> tauri::Result<()> {
    let app = window.app_handle().clone();
    window.with_webview(move |webview| {
        let updater = native
            .as_ref()
            .and_then(|native| native.DisplayUpdater().ok());
        let mut previous = String::new();
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut source = PWSTR::null();
            let mut message = PWSTR::null();
            // I read messages only in the WebView's UI-thread callback and free both COM strings.
            unsafe {
                args.Source(&mut source)?;
            }
            let source = take_pwstr(source);
            unsafe {
                args.TryGetWebMessageAsString(&mut message)?;
            }
            let message = take_pwstr(message);
            let Some(update) = playback::parse(&source, &message) else {
                return Ok(());
            };
            app.state::<AppState>().discord.update(update.clone());
            let (Some(native), Some(updater)) = (native.as_ref(), updater.as_ref()) else {
                return Ok(());
            };
            native.SetIsEnabled(update.active)?;
            native.SetPlaybackStatus(if !update.active {
                MediaPlaybackStatus::Closed
            } else if update.playing {
                MediaPlaybackStatus::Playing
            } else {
                MediaPlaybackStatus::Paused
            })?;
            let metadata = serde_json::to_string(&(
                &update.title,
                &update.artist,
                &update.album,
                &update.artwork,
            ))
            .unwrap();
            if previous != metadata {
                updater.ClearAll()?;
                updater.SetType(MediaPlaybackType::Music)?;
                let properties = updater.MusicProperties()?;
                properties.SetTitle(&HSTRING::from(&update.title))?;
                properties.SetArtist(&HSTRING::from(&update.artist))?;
                properties.SetAlbumTitle(&HSTRING::from(&update.album))?;
                if let Some(uri) = artwork_uri(&update.artwork)
                    && let Ok(image) = RandomAccessStreamReference::CreateFromUri(&uri)
                {
                    updater.SetThumbnail(&image)?;
                }
                updater.Update()?;
                previous = metadata;
            }
            Ok(())
        }));
        let mut token = 0;
        let result = unsafe {
            webview
                .controller()
                .CoreWebView2()
                .and_then(|core| core.add_WebMessageReceived(&handler, &mut token))
        };
        if let Err(error) = result {
            eprintln!("Could not attach the media bridge: {error}");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_require_the_music_origin_and_bounded_metadata() {
        let message = r#"{"kind":"ferric-media","title":"Track","artist":"Artist","album":"","artwork":"","active":true,"playing":true}"#;
        assert!(playback::parse("https://music.youtube.com/", message).is_some());
        for source in [
            "https://music.youtube.com.evil.test/",
            "http://music.youtube.com/",
            "https://music.youtube.com:444/",
        ] {
            assert!(playback::parse(source, message).is_none());
        }
        assert!(playback::parse("https://music.youtube.com/", &"x".repeat(16_385)).is_none());
        assert!(
            playback::parse(
                "https://music.youtube.com/",
                &message.replace("Track", &"x".repeat(2049))
            )
            .is_none()
        );
        assert!(
            playback::parse(
                "https://music.youtube.com/",
                &message.replace("ferric-media", "settings")
            )
            .is_none()
        );
    }

    #[test]
    fn artwork_is_restricted_to_https_image_hosts() {
        assert!(artwork_uri("https://i.ytimg.com/image.jpg").is_some());
        for value in [
            "http://i.ytimg.com/image.jpg",
            "file:///C:/image.png",
            "https://i.ytimg.com.evil.test/image.jpg",
            "https://localhost/image.jpg",
            "https://i.ytimg.com:444/image.jpg",
            "https://user:secret@i.ytimg.com/image.jpg",
        ] {
            assert!(artwork_uri(value).is_none());
        }
    }
}
