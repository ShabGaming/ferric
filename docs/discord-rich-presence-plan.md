# Discord Rich Presence plan

Status: implemented and approved by the maintainer for merge after testing the release preview. Automated checks and live Rust IPC verification passed.
Branch: `feature/discord-rich-presence`.

## Agreed behavior

- Opt-in through a **Discord Rich Presence** checkbox in the existing window Settings menu and tray Settings menu. Default off for new and existing installations.
- Publish the currently playing track. Clear presence when paused, ended, disabled, or when the player closes, reloads, or leaves YouTube Music.
- Continue publishing while music plays with the window hidden in the tray.
- Discord being absent, starting later, or restarting must not interrupt playback or show repeated error dialogs.
- Full card application name: **YouTube Music (Ferric)**. Compact status under the user shows the song title (`status_display_type: 2`, using the `details` field).

## Proposed first version

The Discord card should use a Listening activity, with the track title as details and the artist as state. Show album artwork, album text on hover when available, playback progress for tracks with a finite duration, and a **Listen on YouTube Music** link when a reliable track ID is available. Use a registered Ferric image when artwork is absent or unsupported. Confirm actual rendering in the Discord desktop client before treating this layout as final.

Do not publish an idle card. Omit invalid or unavailable fields. Suppress ads when the player exposes a reliable ad signal; treat ambiguous metadata transitions conservatively so a previous track is not shown for new playback. Clear while buffering if playback progress has stopped, and republish with corrected timestamps on resume.

## Architecture

Reuse the existing `media.js` → WebView2 → Rust bridge. Keep its origin restrictions and payload bounds; do not expose additional general Tauri commands to the remote page.

1. **Shared playback updates.** Extend the bridge with current position, finite duration, playback rate, buffering state, and a reliable current track ID. Do not assume the page URL identifies the playing song while browsing. Validate data in Rust, including finite/range checks for numbers and an allowlisted canonical song URL built from the track ID. Reuse the HTTPS artwork host restrictions.
2. **Independent consumers.** Separate message attachment/validation from Windows media-control initialization. Dispatch a validated playback snapshot to both Windows controls and Discord, so Windows media failures cannot prevent Discord updates. Keep Discord failures from propagating through the Windows media callback.
3. **Discord worker.** Add `discord.rs` with a single background worker that owns the IPC connection and latest desired presence. Use a bounded/coalescing update mechanism so rapid updates cannot grow an unbounded queue. The WebView/UI thread must never wait for Discord I/O.
4. **Connection lifecycle.** Connect only when enabled and there is a track to publish. Retry with capped backoff after connection failures, republishing the latest state after reconnect. Detect disconnects even during an unchanged track. Discard obsolete pending updates after disable, pause, navigation, or player destruction. Clear and disconnect on disable, with bounded shutdown cleanup on exit.
5. **Settings and menus.** Add a backward-compatible `discord_rich_presence` preference to `settings.rs`, defaulting to false. Reuse the existing save-before-applying preference pattern and keep both menus synchronized. Enabling requests a fresh playback snapshot; disabling clears immediately and stops retries.

The implementation uses a small, tested IPC transport on the existing Tokio runtime dependency rather than `discord-rich-presence`. This gives the worker cancellable reads/writes, bounded timeouts, explicit READY and command acknowledgement checks, and ping/disconnect handling. Tokio's macros feature adds one compile-time dependency. No additional runtime service or SDK is required.

The worker retains only the latest desired snapshot using a watch channel, spaces metadata publications at least five seconds apart within a connection, retries failures with a 1–30 second backoff, and expires snapshots after 90 seconds without reconciliation (checked at intervals up to 15 seconds). Clear operations bypass publication spacing and wait at most 500 ms for acknowledgement before disconnecting. Remote artwork rejected by Discord retries with the registered `ferric` asset.

## Update policy

- Publish on track/metadata changes, play/resume, seeks, playback-rate changes, and recovery from buffering.
- Calculate Discord start/end timestamps from position, duration, and playback rate. Omit progress when duration is unknown, nonfinite, or otherwise unusable, including live streams.
- Use event-driven updates and coalesce bursts to respect Discord's limits. Send clear operations promptly. Use low-frequency reconciliation only where events cannot detect stale state; avoid a per-second presence update loop.
- Do not clear merely because the window is hidden. Do clear on full navigation and release the old document's state so it cannot reappear on reconnect.
- Preserve UTF-8 while bounding Discord text fields. Reject malformed links and do not forward profile/account information.

## Implementation order

1. Confirm a Discord application identity and prove the basic Listening card, artwork, link, and progress behavior with the desktop client.
2. Extract shared playback handling and add validated timing/track identity fields without changing Windows media behavior.
3. Implement the worker, presence mapping, retries, coalescing, and clearing lifecycle.
4. Wire the persisted setting and menu toggle; document usage and troubleshooting in the README.
5. Run automated checks and the manual acceptance matrix below.

## Validation

Automated coverage should exercise settings migration, malformed metadata/timestamps/URLs, Unicode truncation, presence mapping, seek/rate calculations, reconnect with latest state, and clearing/disable races using a fake transport. Run the repository's formatting, Clippy, and test commands, then build the release package.

Manual checks:

- Fresh install and existing settings: disabled until explicitly enabled.
- Play, pause, resume, seek, next/previous, repeat, rapid skips, long/non-Latin titles, and missing artwork.
- Live streams, buffering, ads, navigation/reload/sign-out, and browsing away from the playing track.
- Start without Discord; open it later; restart it mid-track; verify current track and accurate progress return.
- Enable and disable while playing or disconnected; ensure stale cards do not reappear.
- Hide to tray, restore, startup into tray without a player, and exit with both close-to-tray settings.
- Windows media buttons, sign-in, normal playback, and background memory behavior remain functional.
- Inspect the actual Discord profile card from another account where needed to verify links and layout.

## Discord application setup

Portal setup reported complete by the maintainer:

- Application/client ID: `1562983781768626278`.
- Fallback artwork asset key: `ferric`.
- Artwork dimensions: 1024 × 1024 pixels.
- Registered application name: **Ferric**. The activity overrides its display name to **YouTube Music (Ferric)**.

Use this application for distributed builds so end users only need the toggle. The public key is not needed for the planned local presence connection. Do not request user tokens or ship application secrets.

## Initial connection test

Run `./scripts/Test-DiscordPresence.ps1` from PowerShell 7 with Discord desktop signed in. It publishes clearly labeled sample text, the `ferric` asset, a Listening activity, progress timestamps, and an **Open YouTube Music** button for 120 seconds, then clears the presence. Use `-Seconds` to change the duration (5–600 seconds). This script is independent of the player and adds no app dependencies. Restricted execution sandboxes may deny access to Discord's named pipe.

The live test on 2026-10-22 confirmed:

- Discord accepted the application handshake and acknowledged `SET_ACTIVITY` with activity type 2 (Listening), without OAuth or bot tokens.
- The maintainer confirmed the sample card's artwork and progress rendered correctly.
- Discord acknowledged clearing the sample presence after the two-minute test; the script exited successfully.
- The production Rust transport also passed an explicit live smoke test: handshake, Listening publication, and clearing were all acknowledged by Discord.
- The live Rust smoke test asserted that Discord returned `name: "YouTube Music (Ferric)"` and `status_display_type: 2` in its accepted activity.
- The maintainer confirmed the real player publishes a working Listening card. A startup error during development was reproduced and fixed: the shortcut updater must release its read-only loaded shortcut before writing a new executable path. A regression test covers switching between release and development paths.
- The player now supplies validated timing, metadata, and current-player video identity. Live song metadata, links, menu behavior, tray playback, and restart acceptance still need a manual pass with the new build. The standalone script's button opens only the homepage.
- Final automated validation: 17 Rust tests passed, three JavaScript bridge tests passed, formatting and Clippy passed, and the explicitly invoked live Discord smoke test passed. The live test remains ignored by default because it requires Discord and briefly changes presence.
- The maintainer approved the final release preview for pushing and merging. The full manual matrix above remains useful for broader regression testing; approval does not imply every listed scenario was separately exercised.

## References

- [Discord RPC and Windows IPC](https://docs.discord.com/developers/topics/rpc)
- [Discord Rich Presence best practices](https://docs.discord.com/developers/rich-presence/best-practices)
- [discord-rich-presence crate](https://docs.rs/discord-rich-presence/latest/discord_rich_presence/)
- [Candidate crate IPC implementation](https://docs.rs/discord-rich-presence/latest/src/discord_rich_presence/discord_ipc.rs.html)
