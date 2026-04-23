// src/main.js
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let autostartEnabled = false;
let proxyAlphaEnabled = false;

async function refresh() {
  try {
    const [status, config] = await Promise.all([
      invoke('get_status'),
      invoke('get_config'),
    ]);
    autostartEnabled = config.autostart;
    proxyAlphaEnabled = !!config.windows_proxy_alpha_enabled;
    renderStatus(status, config);
  } catch (e) {
    showToast('Ошибка получения статуса: ' + e, true);
  }
}

function renderStatus(status, config) {
  const btn = document.getElementById('toggle-btn');
  btn.className = 'toggle' + (status.bypass_enabled ? ' on' : '');

  const label = document.getElementById('status-label');
  const sub = document.getElementById('status-sub');
  if (status.bypass_enabled) {
    label.textContent = 'Активен';
    label.className = 'status-label active';
    sub.textContent = 'RU трафик идёт напрямую';
    document.getElementById('header-icon').textContent = '🔓';
  } else {
    label.textContent = 'Выключен';
    label.className = 'status-label inactive';
    sub.textContent = 'Весь трафик через VPN';
    document.getElementById('header-icon').textContent = '🔒';
  }

  document.getElementById('stat-subnets').textContent =
    status.subnet_count ? status.subnet_count.toLocaleString('ru') : '—';
  document.getElementById('stat-routes').textContent =
    status.active_routes ? status.active_routes.toLocaleString('ru') : '—';
  document.getElementById('stat-gateway').textContent =
    status.gateway || 'не определён';

  const vpnEl = document.getElementById('stat-vpn');
  if (status.vpn_interface) {
    vpnEl.textContent = '● ' + status.vpn_interface;
    vpnEl.className = 'value ok';
  } else {
    vpnEl.textContent = 'не обнаружен';
    vpnEl.className = 'value warn';
  }

  if (status.last_updated) {
    const d = new Date(status.last_updated);
    document.getElementById('last-updated').textContent =
      'Обновлён: ' + d.toLocaleDateString('ru', {
        day: 'numeric', month: 'long', year: 'numeric'
      });
  } else {
    document.getElementById('last-updated').textContent = 'Не обновлялся';
  }

  document.getElementById('schedule-select').value =
    config.update_schedule || 'weekly';

  const ab = document.getElementById('autostart-btn');
  ab.className = 'toggle-small' + (config.autostart ? ' on' : '');

  const proxyAlpha = document.getElementById('proxy-alpha-btn');
  proxyAlpha.className = 'toggle-small alpha' + (config.windows_proxy_alpha_enabled ? ' on' : '');
}

function showLoading(title) {
  document.getElementById('loading-title').textContent = title;
  document.getElementById('loading-banner').className = 'loading-banner visible';
  document.getElementById('toggle-btn').className = 'toggle loading';
  document.getElementById('toggle-btn').onclick = null;
}

function hideLoading() {
  document.getElementById('loading-banner').className = 'loading-banner';
  document.getElementById('toggle-btn').onclick = toggleBypass;
}

function nextFrame() {
  return new Promise(resolve => requestAnimationFrame(resolve));
}

async function toggleBypass() {
  const isOn = document.getElementById('toggle-btn').classList.contains('on');
  showLoading(isOn ? 'Отключается…' : 'Включается…');
  await nextFrame(); // дать браузеру нарисовать баннер до тяжёлого вызова
  try {
    await invoke('toggle_bypass');
    await refresh();
  } catch (e) {
    showToast(String(e), true);
    await refresh();
  } finally {
    hideLoading();
  }
}

async function updateSubnets() {
  const btn = document.getElementById('btn-update');
  btn.disabled = true;
  btn.textContent = 'Загрузка…';
  showLoading('Обновление списка IP…');
  await nextFrame();
  try {
    const count = await invoke('update_subnets');
    showToast('Обновлено: ' + count.toLocaleString('ru') + ' подсетей');
    await refresh();
  } catch (e) {
    showToast(String(e), true);
  } finally {
    btn.disabled = false;
    btn.textContent = 'Обновить';
    hideLoading();
  }
}

async function setSchedule(value) {
  try {
    await invoke('set_update_schedule', { schedule: value });
  } catch (e) {
    showToast(String(e), true);
  }
}

async function clearRoutes() {
  const btn = document.getElementById('btn-clear');
  btn.disabled = true;
  try {
    const removed = await invoke('clear_all_routes');
    showToast('Удалено маршрутов: ' + removed.toLocaleString('ru'));
    await refresh();
  } catch (e) {
    showToast(String(e), true);
  } finally {
    btn.disabled = false;
  }
}

async function toggleAutostart() {
  autostartEnabled = !autostartEnabled;
  try {
    await invoke('set_autostart', { enabled: autostartEnabled });
    const ab = document.getElementById('autostart-btn');
    ab.className = 'toggle-small' + (autostartEnabled ? ' on' : '');
  } catch (e) {
    showToast(String(e), true);
    autostartEnabled = !autostartEnabled;
  }
}

async function toggleProxyAlpha() {
  const btn = document.getElementById('proxy-alpha-btn');
  btn.disabled = true;
  try {
    proxyAlphaEnabled = await invoke('toggle_windows_proxy_alpha');
    btn.className = 'toggle-small alpha' + (proxyAlphaEnabled ? ' on' : '');
    showToast(proxyAlphaEnabled ? 'Proxy-router alpha включен' : 'Proxy-router alpha выключен');
    await refresh();
  } catch (e) {
    showToast(String(e), true);
    await refresh();
  } finally {
    btn.disabled = false;
  }
}

function showToast(msg, isError = false) {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = 'toast show' + (isError ? ' error' : '');
  clearTimeout(el._timer);
  el._timer = setTimeout(() => { el.className = 'toast'; }, 3500);
}

// Listen for network-changed event from backend
listen('network-changed', () => {
  showToast('Сеть изменилась, маршруты обновлены');
  refresh();
});

// Poll status every 5 seconds
setInterval(refresh, 5000);

// Initial load
refresh();
