// First-run wizard: pick the default agent preset, select plugins, then
// apply through the shell commands (set_preset + install_plugin each).

const PRESETS = [
  { id: 'standard', name: '标准', desc: '通用编码智能体：文件、终端、网络与子代理能力均衡（默认）' },
  { id: 'code', name: '代码', desc: '面向软件开发：更强的代码编辑与工程能力' },
  { id: 'minimal', name: '极简', desc: '最小工具集，会话干净、权限面小' },
  { id: 'cordis', name: 'Cordis', desc: '面向编写/调试 Cordis 组合的专用预设' },
];

const PLUGINS = [
  { id: 'dsh-better-sidebar', spec: 'dsh-better-sidebar@latest', name: '增强侧边栏', desc: '文件工作台、真实终端、Git 面板、内嵌浏览器与子代理面板' },
  { id: 'dsh-notification', spec: 'https://github.com/omdsh-dev/dsh-notification/archive/refs/tags/v0.1.3.tar.gz', name: '桌面通知', desc: '回合完成时发送系统通知，可按结果与关键词过滤' },
  { id: 'dsh-session-context-menu', spec: 'github:baihejiangnan/dsh-session-context-menu', name: '更好的右键菜单', desc: '会话/工作区/文本的原生风格右键菜单（桌面端专用）' },
  { id: 'dsh-im', spec: '-w @xmanrui/dsh-im', name: 'IM 机器人（企微/飞书/钉钉/微信/QQ）', desc: '扫码或凭据接入 9 个 IM 频道，手机上也能用 Harness' },
  { id: 'dsh-lark', spec: 'dsh-lark', name: '飞书机器人', desc: '飞书/Lark IM bot 频道插件' },
  { id: 'dsh-qqbot', spec: 'dsh-qqbot', name: 'QQ 机器人', desc: 'QQ Bot 接入 DeepSeek Harness（腾讯官方插件）' },
  { id: 'dsh-vision-router', spec: 'dsh-vision-router', name: '视觉路由', desc: '内置免费视觉链 + 像素级视觉工具（图像问答/OCR/截图等），无需 Python' },
  { id: 'graph-memory', spec: 'github:adoresever/graph-memory', name: '知识图谱记忆', desc: '跨会话记忆召回：知识图谱 + 语义向量检索，本地优先存储，压缩上下文' },
  { id: 'aegis', spec: 'github:ganyuanran/aegis', name: 'Aegis 方法包', desc: 'AI 编码更可靠：基线优先、证据验证、防漂移，减少返工' },
];

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const isWindows = navigator.userAgent.includes('Windows');

let selectedPreset = 'standard';
const selected = new Set(PLUGINS.filter((p) => p.defaultOn).map((p) => p.id));

function renderPresets() {
  const list = document.getElementById('preset-list');
  list.innerHTML = '';
  for (const p of PRESETS) {
    const el = document.createElement('button');
    el.type = 'button';
    el.className = 'preset-card' + (p.id === selectedPreset ? ' selected' : '');
    el.innerHTML = `<strong>${p.name}</strong><span>${p.desc}</span>`;
    el.addEventListener('click', () => {
      selectedPreset = p.id;
      renderPresets();
    });
    list.appendChild(el);
  }
}

function renderPlugins() {
  const list = document.getElementById('plugin-list');
  list.innerHTML = '';
  for (const p of PLUGINS) {
    if (p.winOnly && !isWindows) continue;
    const label = document.createElement('label');
    label.className = 'plugin-row';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = selected.has(p.id);
    cb.disabled = !!p.required;
    cb.addEventListener('change', () => {
      if (cb.checked) selected.add(p.id);
      else selected.delete(p.id);
    });
    const body = document.createElement('span');
    body.className = 'plugin-body';
    body.innerHTML = `<strong>${p.name}</strong><span>${p.desc}</span>`;
    label.append(cb, body);
    list.appendChild(label);
  }
}

const progress = document.getElementById('progress');
const fill = document.getElementById('progress-fill');
const progressText = document.getElementById('progress-text');

function showProgress(text, frac) {
  progress.classList.remove('hidden');
  fill.style.width = `${Math.round((frac ?? 0) * 100)}%`;
  progressText.textContent = text;
}

document.getElementById('btn-finish').addEventListener('click', async () => {
  const btn = document.getElementById('btn-finish');
  btn.disabled = true;
  const steps = [...selected];
  const total = steps.length + 1;
  try {
    showProgress('写入预设配置…', 1 / total);
    await invoke('set_preset', { preset: selectedPreset });
    for (let i = 0; i < steps.length; i++) {
      const plugin = PLUGINS.find((p) => p.id === steps[i]);
      showProgress(`安装插件：${plugin ? plugin.name : steps[i]}…`, (i + 2) / total);
      await invoke('install_plugin', { spec: plugin.spec });
    }
    showProgress('完成', 1);
    // Newly installed plugin bundles load when the dsh service restarts;
    // the shell reconnects and lands on the main UI.
    await invoke('restart_dsh').catch(() => {});
    setTimeout(() => {
      window.location.href = 'index.html';
    }, 400);
  } catch (err) {
    progressText.textContent = `安装失败：${String(err)}`;
    btn.disabled = false;
  }
});

renderPresets();
renderPlugins();
