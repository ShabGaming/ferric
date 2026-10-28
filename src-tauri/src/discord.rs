//! Presence-only Discord IPC. All pipe I/O stays on one background runtime.
use crate::playback::{self, Update};
use serde_json::{Value, json};
use std::{
    io,
    sync::Mutex,
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::ClientOptions,
    sync::watch,
    time::{sleep, timeout},
};

const APPLICATION_ID: &str = "1562983781768626278";
const DISPLAY_NAME: &str = "YouTube Music (Ferric)";
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const UPDATE_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(90);
const END_GRACE: Duration = Duration::from_secs(10);
const MAX_FRAME: usize = 65_536;

#[derive(Clone, Default)]
struct Desired {
    enabled: bool,
    shutdown: bool,
    track: Option<Snapshot>,
}

#[derive(Clone)]
struct Snapshot {
    update: Update,
    received: Instant,
    unix_seconds: i64,
}

impl Snapshot {
    // A hidden page's timers can be throttled, so a steadily playing song stays fresh until it should have ended.
    fn expired(&self) -> bool {
        let elapsed = self.received.elapsed();
        if elapsed < STALE_AFTER {
            return false;
        }
        let update = &self.update;
        update
            .position
            .zip(update.duration)
            .filter(|(position, duration)| *duration > 0.0 && position < duration)
            .is_none_or(|(position, duration)| {
                elapsed.as_secs_f64()
                    >= (duration - position) / update.playback_rate + END_GRACE.as_secs_f64()
            })
    }
}

impl Desired {
    fn activity(&self) -> Option<Activity> {
        let track = self.track.as_ref()?;
        let update = &track.update;
        if !self.enabled
            || self.shutdown
            || track.expired()
            || !update.active
            || !update.playing
            || update.buffering
            || update.ad
            || update.title.trim().is_empty()
        {
            return None;
        }
        let timing = update
            .position
            .zip(update.duration)
            .filter(|(position, duration)| *duration > 0.0 && position < duration);
        let (start, end) = timing.map_or((None, None), |(position, duration)| {
            let start = track.unix_seconds - (position / update.playback_rate).round() as i64;
            (
                Some(start),
                Some(start + (duration / update.playback_rate).round() as i64),
            )
        });
        Some(Activity {
            title: text(&update.title),
            artist: text(&update.artist),
            album: text(&update.album),
            artwork: if playback::valid_artwork(&update.artwork) {
                update.artwork.clone()
            } else {
                "ferric".into()
            },
            track_id: update.track_id.clone(),
            start,
            end,
        })
    }
}

fn text(value: &str) -> String {
    let mut output = String::new();
    for character in value.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if output.len() + character.len_utf8() > 128 {
            break;
        }
        output.push(character);
    }
    output
}

#[derive(Clone, Debug)]
struct Activity {
    title: String,
    artist: String,
    album: String,
    artwork: String,
    track_id: String,
    start: Option<i64>,
    end: Option<i64>,
}

impl Activity {
    fn equivalent(&self, other: &Self) -> bool {
        fn near(left: Option<i64>, right: Option<i64>) -> bool {
            match (left, right) {
                (Some(left), Some(right)) => left.abs_diff(right) <= 2,
                (None, None) => true,
                _ => false,
            }
        }
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.artwork == other.artwork
            && self.track_id == other.track_id
            && near(self.start, other.start)
            && near(self.end, other.end)
    }

    fn payload(&self) -> Value {
        let mut activity = json!({
            "type": 2, "name": DISPLAY_NAME, "details": self.title,
            "status_display_type": 2, // Show the song (details) under the user in member lists.
            "assets": { "large_image": self.artwork, "large_text": if self.album.is_empty() { "Ferric" } else { &self.album } }
        });
        if !self.artist.is_empty() {
            activity["state"] = json!(self.artist);
        }
        if let (Some(start), Some(end)) = (self.start, self.end) {
            activity["timestamps"] = json!({ "start": start, "end": end });
        }
        if playback::valid_track_id(&self.track_id) {
            activity["buttons"] = json!([{
                "label": "Listen on YouTube Music",
                "url": format!("https://music.youtube.com/watch?v={}", self.track_id)
            }]);
        }
        activity
    }
}

pub struct Presence {
    desired: watch::Sender<Desired>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Presence {
    pub fn new(enabled: bool) -> Self {
        let (desired, receiver) = watch::channel(Desired {
            enabled,
            ..Desired::default()
        });
        let thread = std::thread::Builder::new()
            .name("ferric-discord".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(worker(receiver)),
                    Err(error) => eprintln!("Could not start Discord integration: {error}"),
                }
            })
            .map_err(|error| eprintln!("Could not start Discord integration: {error}"))
            .ok();
        Self {
            desired,
            thread: Mutex::new(thread),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.desired.send_modify(|state| {
            state.enabled = enabled;
            state.track = None; // Enabling always requires a fresh player snapshot.
        });
    }

    pub fn update(&self, update: Update) {
        self.desired.send_if_modified(|state| {
            if !state.enabled || state.shutdown {
                return false;
            }
            state.track = Some(Snapshot {
                update,
                received: Instant::now(),
                unix_seconds: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            });
            true
        });
    }

    pub fn clear(&self) {
        self.desired.send_modify(|state| state.track = None);
    }

    pub fn shutdown(&self) {
        self.desired.send_modify(|state| state.shutdown = true);
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn worker(mut desired: watch::Receiver<Desired>) {
    worker_with(&mut desired, || {
        (0..10)
            .find_map(|index| {
                ClientOptions::new()
                    .open(format!(r"\\?\pipe\discord-ipc-{index}"))
                    .ok()
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "Discord is unavailable"))
    })
    .await;
}

async fn worker_with<S: AsyncRead + AsyncWrite + Unpin>(
    desired: &mut watch::Receiver<Desired>,
    mut connect: impl FnMut() -> io::Result<S>,
) {
    let mut retry_at = Instant::now();
    let mut backoff = Duration::from_secs(1);
    loop {
        let current = desired.borrow_and_update().clone();
        if current.shutdown {
            return;
        }
        if current.activity().is_none() {
            if desired.changed().await.is_err() {
                return;
            }
            continue;
        }
        if retry_at > Instant::now() {
            tokio::select! {
                _ = sleep(retry_at.saturating_duration_since(Instant::now())) => {},
                _ = desired.changed() => {},
            }
            continue;
        }
        let result = match connect() {
            Ok(pipe) => connected(Connection::new(pipe), desired).await,
            Err(error) => Err(error),
        };
        if result.is_err() {
            retry_at = Instant::now() + backoff;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        } else {
            retry_at = Instant::now();
            backoff = Duration::from_secs(1);
        }
    }
}

async fn connected<S: AsyncRead + AsyncWrite + Unpin>(
    mut connection: Connection<S>,
    desired: &mut watch::Receiver<Desired>,
) -> io::Result<()> {
    tokio::select! {
        biased;
        _ = desired.changed() => return Ok(()),
        result = bounded(connection.handshake()) => result?,
    }
    let mut previous: Option<Activity> = None;
    let mut rejected_artwork: Option<String> = None;
    let mut published_at = Instant::now() - UPDATE_INTERVAL;
    loop {
        let current = desired.borrow_and_update().clone();
        let Some(mut activity) = current.activity() else {
            // A closed pipe also removes this process's activity if Discord does not reply.
            let _ = timeout(
                Duration::from_millis(500),
                connection.set_activity(Value::Null),
            )
            .await;
            return Ok(());
        };
        if rejected_artwork.as_deref() == Some(activity.artwork.as_str()) {
            activity.artwork = "ferric".into();
        }
        let changed = previous
            .as_ref()
            .is_none_or(|previous| !activity.equivalent(previous));
        if changed && published_at.elapsed() >= UPDATE_INTERVAL {
            tokio::select! {
                biased;
                // Cancel an obsolete write by dropping the entire connection, never reusing a partial frame.
                _ = desired.changed() => return Ok(()),
                result = async {
                    match bounded(connection.set_activity(activity.payload())).await {
                        Err(error) if error.kind() == io::ErrorKind::InvalidInput && activity.artwork != "ferric" => {
                            rejected_artwork = Some(std::mem::replace(&mut activity.artwork, "ferric".into()));
                            bounded(connection.set_activity(activity.payload())).await
                        }
                        result => result,
                    }
                } => { result?; },
            }
            previous = Some(activity);
            published_at = Instant::now();
            continue;
        }
        let wait = if changed {
            UPDATE_INTERVAL.saturating_sub(published_at.elapsed())
        } else {
            Duration::from_secs(15)
        };
        tokio::select! {
            biased;
            _ = desired.changed() => {},
            frame = connection.read_frame() => {
                let (opcode, bytes) = frame?;
                match opcode {
                    3 => bounded(connection.write_frame(4, &bytes)).await?,
                    1 => {
                        let response: Value = serde_json::from_slice(&bytes)?;
                        if response["evt"] == "ERROR" {
                            return Err(io::Error::other("Discord rejected the presence"));
                        }
                    },
                    4 => {},
                    _ => return Err(io::Error::other("Discord disconnected")),
                }
            },
            _ = sleep(wait) => {},
        }
    }
}

async fn bounded<T>(future: impl std::future::Future<Output = io::Result<T>>) -> io::Result<T> {
    timeout(IO_TIMEOUT, future)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Discord IPC timed out"))?
}

struct Connection<S> {
    stream: S,
    incoming: Vec<u8>,
    nonce: u64,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            incoming: Vec::new(),
            nonce: 0,
        }
    }

    async fn write_frame(&mut self, opcode: u32, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_FRAME {
            return Err(io::Error::other("Oversized Discord frame"));
        }
        let mut frame = Vec::with_capacity(8 + bytes.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        frame.extend_from_slice(bytes);
        self.stream.write_all(&frame).await
    }

    async fn write_json(&mut self, opcode: u32, payload: Value) -> io::Result<()> {
        self.write_frame(opcode, &serde_json::to_vec(&payload)?)
            .await
    }

    async fn read_frame(&mut self) -> io::Result<(u32, Vec<u8>)> {
        loop {
            if self.incoming.len() >= 8 {
                let opcode = u32::from_le_bytes(self.incoming[..4].try_into().unwrap());
                let length = u32::from_le_bytes(self.incoming[4..8].try_into().unwrap()) as usize;
                if length > MAX_FRAME {
                    return Err(io::Error::other("Oversized Discord frame"));
                }
                if self.incoming.len() >= 8 + length {
                    let bytes = self.incoming[8..8 + length].to_vec();
                    self.incoming.drain(..8 + length);
                    return Ok((opcode, bytes));
                }
            }
            // read is cancellation safe; partial frames survive player updates in select!.
            let mut buffer = [0; 4096];
            let read = self.stream.read(&mut buffer).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Discord disconnected",
                ));
            }
            self.incoming.extend_from_slice(&buffer[..read]);
        }
    }

    async fn response(&mut self, expected_nonce: Option<&str>) -> io::Result<Value> {
        loop {
            let (opcode, bytes) = self.read_frame().await?;
            if opcode == 3 {
                self.write_frame(4, &bytes).await?;
                continue;
            }
            if opcode == 4 {
                continue;
            }
            if opcode != 1 {
                return Err(io::Error::other("Discord rejected the session"));
            }
            let response: Value = serde_json::from_slice(&bytes)?;
            if response["evt"] == "ERROR" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Discord rejected the presence request",
                ));
            }
            let matches = match expected_nonce {
                Some(nonce) => response["nonce"] == nonce && response["cmd"] == "SET_ACTIVITY",
                None => response["evt"] == "READY",
            };
            if matches {
                return Ok(response);
            }
        }
    }

    async fn handshake(&mut self) -> io::Result<()> {
        self.write_json(0, json!({ "v": 1, "client_id": APPLICATION_ID }))
            .await?;
        self.response(None).await?;
        Ok(())
    }

    async fn set_activity(&mut self, activity: Value) -> io::Result<Value> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.write_json(
            1,
            json!({ "cmd": "SET_ACTIVITY", "args": {
            "pid": std::process::id(), "activity": activity
        }, "nonce": nonce }),
        )
        .await?;
        self.response(Some(&nonce)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired() -> Desired {
        Desired {
            enabled: true,
            shutdown: false,
            track: Some(Snapshot {
                update: playback::tests::track(),
                received: Instant::now(),
                unix_seconds: 1000,
            }),
        }
    }

    async fn ready(server: &mut Connection<tokio::io::DuplexStream>) {
        assert_eq!(bounded(server.read_frame()).await.unwrap().0, 0);
        server.write_json(1, json!({"evt": "READY"})).await.unwrap();
    }

    async fn request(server: &mut Connection<tokio::io::DuplexStream>) -> Value {
        let (opcode, bytes) = bounded(server.read_frame()).await.unwrap();
        assert_eq!(opcode, 1);
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn acknowledge(server: &mut Connection<tokio::io::DuplexStream>, request: &Value) {
        server
            .write_json(
                1,
                json!({"cmd": "SET_ACTIVITY", "nonce": request["nonce"], "data": null}),
            )
            .await
            .unwrap();
    }

    async fn settle(server: &mut Connection<tokio::io::DuplexStream>) {
        // A pong proves the worker consumed the publication acknowledgement and is listening again.
        server.write_frame(3, b"{}").await.unwrap();
        assert_eq!(bounded(server.read_frame()).await.unwrap().0, 4);
    }

    #[test]
    fn enabling_requires_fresh_metadata_and_disabling_discards_updates() {
        let (sender, _receiver) = watch::channel(Desired::default());
        let presence = Presence {
            desired: sender,
            thread: Mutex::new(None),
        };
        presence.update(playback::tests::track());
        assert!(presence.desired.borrow().track.is_none());
        presence.set_enabled(true);
        assert!(presence.desired.borrow().activity().is_none());
        presence.update(playback::tests::track());
        assert!(presence.desired.borrow().activity().is_some());
        presence.set_enabled(false);
        presence.update(playback::tests::track());
        assert!(presence.desired.borrow().track.is_none());
        presence.set_enabled(true);
        assert!(presence.desired.borrow().track.is_none());
    }

    #[tokio::test]
    async fn reconnects_with_latest_track_and_clears_on_disable() {
        use std::{
            collections::VecDeque,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
        };
        let (sender, mut receiver) = watch::channel(desired());
        let (first_client, first_server) = tokio::io::duplex(4096);
        let (second_client, second_server) = tokio::io::duplex(4096);
        let mut clients = VecDeque::from([first_client, second_client]);
        let calls = Arc::new(AtomicUsize::new(0));
        let attempts = calls.clone();
        let task = tokio::spawn(async move {
            worker_with(&mut receiver, move || {
                attempts.fetch_add(1, Ordering::Relaxed);
                clients
                    .pop_front()
                    .ok_or_else(|| io::Error::other("No server"))
            })
            .await;
        });
        let mut first = Connection::new(first_server);
        ready(&mut first).await;
        let initial = request(&mut first).await;
        acknowledge(&mut first, &initial).await;
        settle(&mut first).await;
        drop(first); // Simulate Discord restarting during an unchanged track.
        sender.send_modify(|state| {
            state.track.as_mut().unwrap().update.title = "Intermediate".into()
        });
        sender.send_modify(|state| state.track.as_mut().unwrap().update.title = "Latest".into());
        let mut second = Connection::new(second_server);
        ready(&mut second).await;
        let latest = request(&mut second).await;
        assert_eq!(latest["args"]["activity"]["details"], "Latest");
        acknowledge(&mut second, &latest).await;
        settle(&mut second).await;
        sender.send_modify(|state| state.enabled = false);
        let clear = request(&mut second).await;
        assert!(clear["args"]["activity"].is_null());
        acknowledge(&mut second, &clear).await;
        sender.send_modify(|state| state.shutdown = true);
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn rejected_external_artwork_retries_with_registered_fallback() {
        let (sender, mut receiver) = watch::channel(desired());
        let (client, server) = tokio::io::duplex(4096);
        let task =
            tokio::spawn(async move { connected(Connection::new(client), &mut receiver).await });
        let mut server = Connection::new(server);
        ready(&mut server).await;
        let external = request(&mut server).await;
        assert_eq!(
            external["args"]["activity"]["assets"]["large_image"],
            "https://i.ytimg.com/image.jpg"
        );
        server
            .write_json(1, json!({"evt": "ERROR", "nonce": external["nonce"]}))
            .await
            .unwrap();
        let fallback = request(&mut server).await;
        assert_eq!(
            fallback["args"]["activity"]["assets"]["large_image"],
            "ferric"
        );
        acknowledge(&mut server, &fallback).await;
        settle(&mut server).await;
        sender.send_modify(|state| state.shutdown = true);
        let clear = request(&mut server).await;
        assert!(clear["args"]["activity"].is_null());
        acknowledge(&mut server, &clear).await;
        assert!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }

    #[tokio::test]
    #[ignore = "requires Discord desktop; temporarily publishes a sample Listening activity"]
    async fn live_discord_smoke_test() {
        let pipe = (0..10)
            .find_map(|index| {
                ClientOptions::new()
                    .open(format!(r"\\?\pipe\discord-ipc-{index}"))
                    .ok()
            })
            .expect("Open Discord desktop first");
        let mut connection = Connection::new(pipe);
        bounded(connection.handshake()).await.unwrap();
        let mut state = desired();
        let track = state.track.as_mut().unwrap();
        track.unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        track.update.title = "Ferric implementation test".into();
        track.update.artist = "Sample activity".into();
        track.update.artwork.clear();
        track.update.track_id.clear();
        let published = bounded(connection.set_activity(state.activity().unwrap().payload())).await;
        let cleared = bounded(connection.set_activity(Value::Null)).await;
        let published = published.unwrap();
        assert_eq!(published["data"]["name"], DISPLAY_NAME);
        assert_eq!(published["data"]["status_display_type"], 2);
        cleared.unwrap();
        eprintln!(
            "Discord acknowledged the Rust handshake, display name, song status, Listening activity, and clear."
        );
    }

    #[test]
    fn maps_metadata_timing_artwork_and_canonical_links() {
        let mut state = desired();
        let activity = state.activity().unwrap();
        assert_eq!((activity.start, activity.end), (Some(970), Some(1150)));
        assert_eq!(activity.payload()["type"], 2);
        assert_eq!(activity.payload()["name"], DISPLAY_NAME);
        assert_eq!(activity.payload()["status_display_type"], 2);
        assert_eq!(
            activity.payload()["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=abcdefgh_-1"
        );
        let track = state.track.as_mut().unwrap();
        track.update.playback_rate = 2.0;
        track.update.artwork = "https://evil.test/image.png".into();
        track.update.track_id.clear();
        let activity = state.activity().unwrap();
        assert_eq!((activity.start, activity.end), (Some(985), Some(1075)));
        assert_eq!(activity.artwork, "ferric");
        assert!(activity.payload().get("buttons").is_none());
        state.track.as_mut().unwrap().update.duration = None;
        assert!(
            state
                .activity()
                .unwrap()
                .payload()
                .get("timestamps")
                .is_none()
        );
    }

    #[test]
    fn clears_paused_buffering_ads_stale_disabled_and_shutdown_states() {
        for condition in 0..8 {
            let mut state = desired();
            let track = state.track.as_mut().unwrap();
            match condition {
                0 => track.update.playing = false,
                1 => track.update.active = false,
                2 => track.update.buffering = true,
                3 => track.update.ad = true,
                4 => {
                    track.received -= STALE_AFTER;
                    track.update.duration = None;
                }
                5 => track.update.title = "  ".into(),
                6 => state.enabled = false,
                _ => state.shutdown = true,
            }
            assert!(state.activity().is_none());
        }
        let bounded = text(&"界".repeat(100));
        assert!(bounded.len() <= 128);
        assert!(!text("a\nb").contains('\n'));
    }

    #[test]
    fn quiet_pages_stay_fresh_until_the_song_should_have_ended() {
        let mut state = desired();
        let track = state.track.as_mut().unwrap();
        track.received -= STALE_AFTER + Duration::from_secs(10); // 150s of the song remained when it was reported.
        assert!(state.activity().is_some());
        let track = state.track.as_mut().unwrap();
        track.received -= Duration::from_secs(70);
        assert!(state.activity().is_none());
        let track = state.track.as_mut().unwrap();
        track.received = Instant::now() - STALE_AFTER;
        track.update.duration = None;
        assert!(state.activity().is_none());
    }

    #[tokio::test]
    async fn validates_handshake_errors_and_handles_ping() {
        let (client, server) = tokio::io::duplex(4096);
        let mut client = Connection::new(client);
        let server = tokio::spawn(async move {
            let mut server = Connection::new(server);
            let (opcode, bytes) = server.read_frame().await.unwrap();
            assert_eq!(opcode, 0);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["client_id"],
                APPLICATION_ID
            );
            server.write_json(3, json!({"ping": 1})).await.unwrap();
            assert_eq!(server.read_frame().await.unwrap().0, 4);
            server.write_json(1, json!({"evt": "ERROR"})).await.unwrap();
        });
        assert!(bounded(client.handshake()).await.is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pause_cancels_pending_publication_and_drops_connection() {
        let (sender, mut receiver) = watch::channel(desired());
        let (client, server) = tokio::io::duplex(4096);
        let task =
            tokio::spawn(async move { connected(Connection::new(client), &mut receiver).await });
        let mut server = Connection::new(server);
        server.read_frame().await.unwrap();
        server.write_json(1, json!({"evt": "READY"})).await.unwrap();
        let (_, bytes) = server.read_frame().await.unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["args"]["activity"]["details"], "Track");
        // No acknowledgement: pause must still cancel the in-flight request immediately.
        sender.send_modify(|state| state.track = None);
        assert!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        assert!(server.read_frame().await.is_err());
        assert!(sender.borrow().activity().is_none());
    }

    #[tokio::test]
    async fn retains_fragmented_frames_when_reads_are_cancelled() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut client = Connection::new(client);
        server.write_all(&1_u32.to_le_bytes()).await.unwrap();
        assert!(
            timeout(Duration::from_millis(10), client.read_frame())
                .await
                .is_err()
        );
        server.write_all(&2_u32.to_le_bytes()).await.unwrap();
        server.write_all(b"{}").await.unwrap();
        assert_eq!(
            bounded(client.read_frame()).await.unwrap(),
            (1, b"{}".to_vec())
        );
        server.write_all(&1_u32.to_le_bytes()).await.unwrap();
        server.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        assert!(bounded(client.read_frame()).await.is_err());
    }
}
