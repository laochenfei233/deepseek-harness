// Shell page logic: first-run gate, dsh lifecycle events, and the
// host <-> iframe bridge (native notifications and default-browser links).
// The top navigation bar is intentionally gone; the dsh UI owns its chrome.
// The iframe stays blank until Rust exchanges dsh's one-time launch token for
// a WebView cookie; loading the token URL directly would leave the strict
// browser cookie unusable inside this cross-origin iframe.

const frame = document.getElementById('dsh-frame');
const banner = document.getElementById('error-banner');
const errorText = document.getElementById('error-text');
let activeUrl;
let pendingUrl;

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const listen = (event, cb) => window.__TAURI__.event.listen(event, cb);

function showError(text) {
  errorText.textContent = text;
  banner.classList.remove('hidden');
}

function hideError() {
  banner.classList.add('hidden');
}

// Exchanges the launch URL through Rust, which installs the browser cookie in
// the native WebView store, then loads the clean URL it returns.
async function openAuthenticated(url) {
  if (!url || url === activeUrl || url === pendingUrl) return;
  pendingUrl = url;
  try {
    const session = await invoke('authenticate_webview', { url });
    if (session.url === activeUrl) return;
    hideError();
    activeUrl = session.url;
    frame.src = session.url;
  } catch (err) {
    showError(String(err ?? 'dsh 服务启动失败'));
  } finally {
    pendingUrl = undefined;
  }
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
  activeUrl = undefined;
  pendingUrl = undefined;
  frame.src = 'about:blank';
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
    // Tauri bridge unavailable (plain browser preview): leave the iframe blank
    // because only the Rust shell can learn the one-time launch token.
    bridge = false;
  }
  if (!bridge) return;

  listen('dsh://ready', (e) => {
    openAuthenticated(e.payload?.url);
  });
  listen('dsh://failed', (e) => {
    showError(String(e.payload ?? 'dsh 服务启动失败'));
  });

  // Polling also recovers from a missed ready event and turns a hub that never
  // becomes authenticated into a visible error instead of a blank window.
  for (let round = 0; round < 30; round++) {
    await new Promise((resolve) => setTimeout(resolve, 2000));
    try {
      const status = await invoke('dsh_status');
      if (status.running && status.url) {
        await openAuthenticated(status.url);
        if (activeUrl) return;
      }
    } catch {
      // dsh_status unavailable; keep polling until the round budget ends.
    }
  }
  showError('dsh 服务启动失败');
})();