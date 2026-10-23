use serde::Deserialize;
use tauri::Url;

pub const MAX_SECONDS: f64 = 604_800.0;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    kind: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub artwork: String,
    pub active: bool,
    pub playing: bool,
    #[serde(default)]
    pub buffering: bool,
    #[serde(default)]
    pub ad: bool,
    #[serde(default)]
    pub track_id: String,
    #[serde(default)]
    pub position: Option<f64>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default = "normal_rate")]
    pub playback_rate: f64,
}

fn normal_rate() -> f64 {
    1.0
}

pub fn parse(source: &str, message: &str) -> Option<Update> {
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
        || (!update.track_id.is_empty() && !valid_track_id(&update.track_id))
        || [update.position, update.duration]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || !(0.0..=MAX_SECONDS).contains(&value))
        || !update.playback_rate.is_finite()
        || !(0.0625..=16.0).contains(&update.playback_rate)
    {
        return None;
    }
    Some(update)
}

pub fn valid_track_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn valid_artwork(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && ["ytimg.com", "googleusercontent.com", "ggpht.com"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn track() -> Update {
        parse(
            "https://music.youtube.com/",
            &json!({
                "kind": "ferric-media", "title": "Track", "artist": "Artist", "album": "Album",
                "artwork": "https://i.ytimg.com/image.jpg", "active": true, "playing": true,
                "track_id": "abcdefgh_-1", "position": 30.0, "duration": 180.0, "playback_rate": 1.0
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn requires_music_origin_and_bounded_metadata() {
        let message = r#"{"kind":"ferric-media","title":"Track","artist":"Artist","album":"","artwork":"","active":true,"playing":true}"#;
        assert!(parse("https://music.youtube.com/", message).is_some());
        for source in [
            "https://music.youtube.com.evil.test/",
            "http://music.youtube.com/",
            "https://music.youtube.com:444/",
        ] {
            assert!(parse(source, message).is_none());
        }
        for invalid in [
            "x".repeat(16_385),
            message.replace("Track", &"x".repeat(2049)),
            message.replace("ferric-media", "settings"),
        ] {
            assert!(parse("https://music.youtube.com/", &invalid).is_none());
        }
    }

    #[test]
    fn rejects_invalid_timing_and_track_ids() {
        let base = json!({"kind":"ferric-media","title":"Track","artist":"Artist","album":"","artwork":"","active":true,"playing":true});
        for (key, value) in [
            ("position", json!(-1)),
            ("duration", json!(1e100)),
            ("playback_rate", json!(0)),
            ("playback_rate", json!(17)),
            ("track_id", json!("abc&redirect=evil")),
        ] {
            let mut message = base.clone();
            message[key] = value;
            assert!(parse("https://music.youtube.com/", &message.to_string()).is_none());
        }
        assert!(valid_track_id("abcdefgh_-1"));
        assert!(!valid_track_id("ébcdefghij"));
    }

    #[test]
    fn artwork_is_restricted_to_https_image_hosts() {
        assert!(valid_artwork("https://i.ytimg.com/image.jpg"));
        for value in [
            "http://i.ytimg.com/image.jpg",
            "file:///C:/image.png",
            "https://i.ytimg.com.evil.test/image.jpg",
            "https://localhost/image.jpg",
            "https://i.ytimg.com:444/image.jpg",
            "https://user:secret@i.ytimg.com/image.jpg",
        ] {
            assert!(!valid_artwork(value));
        }
    }
}
