(() => {
  if (location.origin !== 'https://music.youtube.com' || window !== window.top) return;
  const session = navigator.mediaSession;
  if (!session || window.__ferricMediaAction) return;
  const actions = new Map();
  let queued = false;
  let previous = '';
  const send = () => {
    queued = false;
    const video = document.querySelector('video, audio');
    const metadata = session.metadata;
    const payload = {
      kind: 'ferric-media',
      title: String(metadata?.title || '').slice(0, 512),
      artist: String(metadata?.artist || '').slice(0, 512),
      album: String(metadata?.album || '').slice(0, 512),
      artwork: String(metadata?.artwork?.at(-1)?.src || '').slice(0, 2048),
      active: !!metadata || !!video && (!video.paused || video.currentTime > 0),
      playing: video ? !video.paused && !video.ended : session.playbackState === 'playing',
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
  for (const event of ['play', 'pause', 'ended', 'emptied', 'loadedmetadata']) {
    document.addEventListener(event, notify, true);
  }
  document.addEventListener('DOMContentLoaded', notify, { once: true });
  notify();
})();
