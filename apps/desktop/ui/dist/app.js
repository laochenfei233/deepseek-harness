// Shell page logic: first-run gate, dsh lifecycle events, and the
// host <-> iframe bridge (native notifications and default-browser links).
// The top navigation bar is intentionally gone; the dsh UI owns its chrome.

const DSH_ORIGIN = 'http://127.0.0.1:3081';

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

function loadUi() {
  frame.src = DSH_ORIGIN;
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
  try {
    const { initialized } = await invoke('first_run_state');
    if (!initialized) {
      window.location.href = 'wizard.html';
      return;
    }
  } catch {
    // Tauri bridge unavailable (plain browser preview): show the UI anyway.
  }

  loadUi();

  listen('dsh://ready', () => {
    hideError();
    loadUi();
  });
  listen('dsh://failed', (e) => {
    showError(String(e.payload ?? 'dsh 服务启动失败'));
  });
})();
