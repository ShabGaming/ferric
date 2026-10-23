(() => {
  if (location.origin !== 'https://music.youtube.com' || window !== window.top) return;
  const session = navigator.mediaSession;
  if (!session || window.__ferricMediaAction) return;
  const actions = new Map();
  let queued = false;
  let previous = '';
  let buffering = false;
  let loading = false;
  let observedPlayer;
  const playerObserver = new MutationObserver(() => notify());
  const send = () => {
    queued = false;
    const video = document.querySelector('video, audio');
    const metadata = session.metadata;
    const player = document.querySelector('#movie_player');
    if (player && player !== observedPlayer) {
      playerObserver.disconnect();
      playerObserver.observe(player, { attributes: true, attributeFilter: ['class'] });
      observedPlayer = player;
    }
    let details;
    try { details = player?.getPlayerResponse?.()?.videoDetails; } catch {}
    const title = String(metadata?.title || '').slice(0, 512);
    // The playing video can differ from the page URL while browsing playlists/artists.
    const matches = !details?.title || details.title.trim() === title.trim();
    const trackId = matches && /^[A-Za-z0-9_-]{11}$/.test(details?.videoId || '') ? details.videoId : '';
    const seconds = value => Number.isFinite(value) && value >= 0 && value <= 604800 ? value : null;
    const rate = video?.playbackRate || 1;
    const payload = {
      kind: 'ferric-media',
      title,
      artist: String(metadata?.artist || '').slice(0, 512),
      album: String(metadata?.album || '').slice(0, 512),
      artwork: String(metadata?.artwork?.at(-1)?.src || '').slice(0, 2048),
      active: !!metadata || !!video && (!video.paused || video.currentTime > 0),
      playing: video ? !video.paused && !video.ended && !video.error : session.playbackState === 'playing',
      buffering: buffering || loading || !!video?.seeking || !matches,
      ad: !!player?.classList.contains('ad-showing'),
      track_id: trackId,
      position: seconds(video?.currentTime),
      duration: seconds(video?.duration),
      playback_rate: Number.isFinite(rate) && rate >= 0.0625 && rate <= 16 ? rate : 1,
    };
    const serialized = JSON.stringify(payload);
    if (serialized === previous) return;
    previous = serialized;
    window.chrome.webview.postMessage(serialized);
  };
  const notify = () => {
    if (queued) return;
    queued = true;
    queueMicrotask(send);
  };
  window.__ferricMediaRefresh = () => { previous = ''; notify(); };
  for (const name of ['metadata', 'playbackState']) {
    const descriptor = Object.getOwnPropertyDescriptor(MediaSession.prototype, name);
    if (!descriptor?.get || !descriptor.set) continue;
    Object.defineProperty(session, name, {
      configurable: true,
      get: () => descriptor.get.call(session),
      set: value => { descriptor.set.call(session, value); notify(); },
    });
  }
  const original = session.setActionHandler.bind(session);
  session.setActionHandler = (action, handler) => {
    original(action, handler);
    if (handler) actions.set(action, handler);
    else actions.delete(action);
  };
  window.__ferricMediaAction = action => {
    if (location.origin !== 'https://music.youtube.com') return;
    const handler = actions.get(action);
    if (handler) { handler({ action }); return; }
    const video = document.querySelector('video, audio');
    if (action === 'play') video?.play().catch(() => {});
    else if (action === 'pause') video?.pause();
    else if (action === 'nexttrack') document.querySelector('ytmusic-player-bar .next-button')?.click();
    else if (action === 'previoustrack') document.querySelector('ytmusic-player-bar .previous-button')?.click();
  };
  for (const event of ['play', 'pause', 'ended', 'error', 'durationchange', 'ratechange', 'seeking', 'seeked']) {
    document.addEventListener(event, notify, true);
  }
  for (const event of ['waiting', 'stalled', 'playing', 'timeupdate', 'emptied', 'loadstart', 'loadedmetadata']) {
    document.addEventListener(event, event => {
      if (!(event.target instanceof HTMLMediaElement)) return;
      if (event.type === 'timeupdate') {
        if (!buffering || event.target.readyState < 3 || event.target.paused) return;
        buffering = false;
      } else if (event.type === 'emptied' || event.type === 'loadstart') {
        loading = true;
        buffering = false;
      } else if (event.type === 'loadedmetadata') {
        loading = false;
      } else {
        buffering = event.type !== 'playing';
        if (event.type === 'playing') loading = false;
      }
      notify();
    }, true);
  }
  // Reconcile missed metadata/ad events and detect abandoned playback without per-second IPC.
  setInterval(() => {
    const video = document.querySelector('video, audio');
    if (video && !video.paused && !video.ended) notify();
  }, 15000);
  document.addEventListener('DOMContentLoaded', notify, { once: true });
  notify();
})();
