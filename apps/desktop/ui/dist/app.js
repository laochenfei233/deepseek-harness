// Shell page logic: first-run gate, dsh lifecycle events, and the
// host <-> iframe navigation bridge (protocol defined by the dsh-tauri
// plugin: sends dsh://sidebar:toggle|page:prev|page:next with
// source 'dsh-desktop', receives dsh-nav-bridge events).

const DSH_ORIGIN = 'http://127.0.0.1:3081';

const frame = document.getElementById('dsh-frame');
const btnSidebar = document.getElementById('btn-sidebar');
const btnBack = document.getElementById('btn-back');
const btnForward = document.getElementById('btn-forward');
const statusDot = document.getElementById('status-dot');
const statusText = document.getElementById('status-text');
const banner = document.getElementById('error-banner');
const errorText = document.getElementById('error-text');

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const listen = (event, cb) => window.__TAURI__.event.listen(event, cb);

function setStatus(kind, text) {
  statusDot.className = 'status-dot ' + kind;
  statusText.textContent = text;
}

function sendToDsh(type) {
  const iframe = frame.contentWindow;
  if (!iframe) return;
  iframe.postMessage({ source: 'dsh-desktop', type }, DSH_ORIGIN);
}

function showError(text) {
  errorText.textContent = text;
  banner.classList.remove('hidden');
}

function hideError() {
  banner.classList.add('hidden');
}

function loadUi() {
  frame.src = DSH_ORIGIN;
  setStatus('loading', '连接中…');
}

function handleBridge(event) {
  if (!event.data || event.data.source !== 'dsh-nav-bridge') return;
  const { type, payload } = event.data;
  if (type === 'dsh://sidebar:collapsed') {
    btnSidebar.classList.toggle('is-active', !!payload?.collapsed);
  } else if (type === 'dsh://page:firsted') {
    btnBack.disabled = !!payload?.firsted;
  } else if (type === 'dsh://page:lasted') {
    btnForward.disabled = !!payload?.lasted;
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

btnSidebar.addEventListener('click', () => sendToDsh('dsh://sidebar:toggle'));
btnBack.addEventListener('click', () => sendToDsh('dsh://page:prev'));
btnForward.addEventListener('click', () => sendToDsh('dsh://page:next'));
document.getElementById('btn-retry').addEventListener('click', () => {
  hideError();
  invoke('restart_dsh');
  setStatus('loading', '正在重启…');
});

window.addEventListener('message', handleBridge);
window.addEventListener('message', handleHostBridge);
frame.addEventListener('load', () => setStatus('ok', '已连接'));

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
    setStatus('ok', '已连接');
    loadUi();
  });
  listen('dsh://restarting', () => setStatus('loading', '正在重启服务…'));
  listen('dsh://restarted-by-plugin', () => setStatus('ok', '服务已由插件重启'));
  listen('dsh://failed', (e) => {
    setStatus('error', '服务异常');
    showError(String(e.payload ?? 'dsh 服务启动失败'));
  });
})();
