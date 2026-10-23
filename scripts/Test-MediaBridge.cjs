// Run with node --test scripts/Test-MediaBridge.cjs. No npm dependencies.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { runInNewContext } = require('node:vm');
const source = readFileSync(join(__dirname, '../src-tauri/src/media.js'), 'utf8');

function bridge(origin = 'https://music.youtube.com') {
  class MediaSession {
    get metadata() { return this.data; }
    set metadata(value) { this.data = value; }
    get playbackState() { return this.state; }
    set playbackState(value) { this.state = value; }
    setActionHandler() {}
  }
  class HTMLMediaElement {}
  const video = Object.assign(new HTMLMediaElement(), {
    paused: false, ended: false, currentTime: 30, duration: 180,
    playbackRate: 1, readyState: 4, seeking: false, error: null,
  });
  const session = new MediaSession();
  session.metadata = { title: 'Song', artist: 'Artist', album: 'Album', artwork: [{ src: 'https://i.ytimg.com/art.jpg' }] };
  let details = { title: 'Song', videoId: 'abcdefgh_-1' };
  let ad = false;
  let mutation;
  let heartbeat;
  const player = {
    getPlayerResponse: () => ({ videoDetails: details }),
    classList: { contains: () => ad },
  };
  const messages = [];
  const queued = [];
  const events = new Map();
  const document = {
    querySelector: selector => selector === '#movie_player' ? player : video,
    addEventListener: (name, handler) => events.set(name, [...(events.get(name) || []), handler]),
  };
  const window = { chrome: { webview: { postMessage: message => messages.push(JSON.parse(message)) } } };
  window.top = window;
  runInNewContext(source, {
    window, document, location: { origin }, navigator: { mediaSession: session },
    MediaSession, HTMLMediaElement,
    MutationObserver: class { constructor(handler) { mutation = handler; } observe() {} disconnect() {} },
    queueMicrotask: callback => queued.push(callback),
    setInterval: callback => { heartbeat = callback; },
  });
  const flush = () => { while (queued.length) queued.shift()(); return messages.at(-1); };
  flush();
  return {
    video, session, messages, flush,
    event(name) { for (const handler of events.get(name) || []) handler({ type: name, target: video }); return flush(); },
    ad(value) { ad = value; mutation(); return flush(); },
    details(value) { details = value; },
    heartbeat() { heartbeat(); return flush(); },
  };
}

test('publishes playing identity and timing independently of the browsing URL', () => {
  const app = bridge();
  assert.equal(app.flush().track_id, 'abcdefgh_-1');
  assert.equal(app.flush().position, 30);
  assert.equal(app.flush().duration, 180);
  app.details({ title: 'Different song', videoId: '12345678901' });
  assert.equal(app.heartbeat().track_id, '');
  assert.equal(app.flush().buffering, true);
  app.session.metadata = { title: 'Different song', artist: 'New artist' };
  assert.equal(app.flush().track_id, '12345678901');
  assert.equal(app.flush().buffering, false);
});

test('tracks pauses, seeks, buffering, ads, errors, and live duration', () => {
  const app = bridge();
  app.video.paused = true;
  assert.equal(app.event('pause').playing, false);
  app.video.paused = false;
  assert.equal(app.event('playing').playing, true);
  assert.equal(app.event('waiting').buffering, true);
  assert.equal(app.event('playing').buffering, false);
  assert.equal(app.ad(true).ad, true);
  assert.equal(app.ad(false).ad, false);
  app.video.seeking = true;
  assert.equal(app.event('seeking').buffering, true);
  app.video.seeking = false;
  app.video.currentTime = 90;
  assert.equal(app.event('seeked').position, 90);
  assert.equal(app.flush().buffering, false);
  app.video.duration = Infinity;
  assert.equal(app.event('durationchange').duration, null);
  app.video.error = {};
  assert.equal(app.event('error').playing, false);
});

test('refreshes unchanged metadata on request and ignores nonmusic origins', () => {
  const app = bridge();
  app.video.currentTime += 15;
  assert.equal(app.heartbeat().position, 45);
  const other = bridge('https://accounts.google.com');
  assert.equal(other.messages.length, 0);
});
