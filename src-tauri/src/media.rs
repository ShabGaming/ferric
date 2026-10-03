use serde::Deserialize;
use tauri::{Url, WebviewWindow};
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    kind: String,
    title: String,
    artist: String,
    album: String,
    artwork: String,
    active: bool,
    playing: bool,
}

fn parse(source: &str, message: &str) -> Option<Update> {
    if Url::parse(source).ok()?.origin().ascii_serialization() != "https://music.youtube.com"
        || message.len() > 16_384
    {
        return None;
    }
    let update: Update = serde_json::from_str(message).ok()?;
    if update.kind != "ferric-media"
        || [
            &update.title,
            &update.artist,
            &update.album,
            &update.artwork,
        ]
        .iter()
        .any(|field| field.len() > 2048)
    {
        return None;
    }
    Some(update)
}

fn artwork_uri(value: &str) -> Option<Uri> {
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || !["ytimg.com", "googleusercontent.com", "ggpht.com"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
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
    let native = controls.clone();
    window.with_webview(move |webview| {
        let updater = match native.DisplayUpdater() {
            Ok(updater) => updater,
            Err(error) => {
                eprintln!("Could not initialize media metadata: {error}");
                return;
            }
        };
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
            let Some(update) = parse(&source, &message) else {
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
    })?;
    Ok(MediaSession {
        controls,
        button_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_require_the_music_origin_and_bounded_metadata() {
        let message = r#"{"kind":"ferric-media","title":"Track","artist":"Artist","album":"","artwork":"","active":true,"playing":true}"#;
        assert!(parse("https://music.youtube.com/", message).is_some());
        for source in [
            "https://music.youtube.com.evil.test/",
            "http://music.youtube.com/",
            "https://music.youtube.com:444/",
        ] {
            assert!(parse(source, message).is_none());
        }
        assert!(parse("https://music.youtube.com/", &"x".repeat(16_385)).is_none());
        assert!(
            parse(
                "https://music.youtube.com/",
                &message.replace("Track", &"x".repeat(2049))
            )
            .is_none()
        );
        assert!(
            parse(
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
