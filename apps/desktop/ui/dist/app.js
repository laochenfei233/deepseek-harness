// Shell page logic: first-run gate, dsh lifecycle events, and the
// host <-> iframe bridge (native notifications and default-browser links).
// The top navigation bar is intentionally gone; the dsh UI owns its chrome.
// The hub port is delivered by the Rust side through dsh://ready (it picks a
// free port when the default 3081 is occupied); the constant below is only
// the plain-browser fallback.

const DEFAULT_ORIGIN = 'http://127.0.0.1:3081';

const frame = document.getElementById('dsh-frame');
const banner = document.getElementById('error-banner');
const errorText = document.getElementById('error-text');

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const listen = (event, cb) => window.__TAURI__.event.listen(event, cb);

function showError(text) {
  errorText.textContent = text;
  banner.classList.remove('hidden');
}

function hideError() {
  banner.classList.add('hidden');
}

function loadUi(port) {
  frame.src = port === undefined ? DEFAULT_ORIGIN : 'http://127.0.0.1:' + port;
}

// The dsh iframe's injected host bridge posts here (source
// 'dsh-desktop-host'): show native OS toasts through tauri-plugin-notification
// and open external links in the default browser through the opener plugin.
function handleHostBridge(event) {
  if (!event.data || event.data.source !== 'dsh-desktop-host') return;
  if (event.data.kind === 'notify') {
    invoke('plugin:notification|notify', {
      options: { title: event.data.title, body: event.data.body },
    }).catch((err) => console.error('[desktop] notification failed:', err));
  } else if (event.data.kind === 'open') {
    invoke('plugin:opener|open_url', { url: event.data.url })
      .catch((err) => console.error('[desktop] open failed:', err));
  }
}

document.getElementById('btn-retry').addEventListener('click', () => {
  hideError();
  invoke('restart_dsh');
});

window.addEventListener('message', handleHostBridge);

(async () => {
  let bridge = true;
  try {
    const { initialized } = await invoke('first_run_state');
    if (!initialized) {
      window.location.href = 'wizard.html';
      return;
    }
  } catch {
    // Tauri bridge unavailable (plain browser preview): show the UI anyway.
    bridge = false;
  }

  if (!bridge) {
    loadUi();
    return;
  }

  // The hub may already be up (surviving restarts): ask for its port instead
  // of waiting for the next dsh://ready.
  try {
    const status = await invoke('dsh_status');
    if (status.running) loadUi(status.port);
  } catch {
    // dsh_status unavailable; the ready event below covers this.
  }

  listen('dsh://ready', (e) => {
    hideError();
    loadUi(e.payload);
  });
  listen('dsh://restarted-by-plugin', (e) => loadUi(e.payload));
  listen('dsh://failed', (e) => {
    showError(String(e.payload ?? 'dsh 服务启动失败'));
  });
})();
