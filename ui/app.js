// ProxyMan Frontend Application Controller

let allServices = [];
let activeFilter = 'all';
let logSocket = null;
let eventSocket = null;
const serviceErrors = {};
const serviceAdopted = {};

// Initialize
document.addEventListener('DOMContentLoaded', () => {
  setupEventListeners();
  loadServices();
  initEventWebSocket();
});

function setupEventListeners() {
  document.getElementById('service-search').addEventListener('input', renderServices);

  document.querySelectorAll('.filter-chip').forEach(chip => {
    chip.addEventListener('click', (e) => {
      document.querySelectorAll('.filter-chip').forEach(c => c.classList.remove('active'));
      e.target.classList.add('active');
      activeFilter = e.target.getAttribute('data-filter');
      renderServices();
    });
  });

  document.getElementById('btn-add-service').addEventListener('click', () => openAddModal());
  document.getElementById('btn-start-all').addEventListener('click', startAllServices);
  document.getElementById('btn-stop-all').addEventListener('click', stopAllServices);
}

// WebSocket for real-time state events
function initEventWebSocket() {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsUrl = `${protocol}//${window.location.host}/api/events/ws`;

  eventSocket = new WebSocket(wsUrl);

  eventSocket.onmessage = (event) => {
    try {
      const state = JSON.parse(event.data);
      updateServiceState(state);
    } catch (err) {
      console.error('Error parsing event websocket message', err);
    }
  };

  eventSocket.onclose = () => {
    setTimeout(initEventWebSocket, 3000);
  };
}

function updateServiceState(runtimeState) {
  const idx = allServices.findIndex(s => s.id === runtimeState.id);
  if (idx !== -1) {
    allServices[idx].runtime = runtimeState;
    renderServices();
    updateStats();
  }
}

// REST API calls
async function loadServices() {
  try {
    const res = await fetch('/api/services');
    if (res.ok) {
      allServices = (await res.json()).map(s => ({
        ...s,
        service_type: normalizeServiceType(s.service_type),
      }));
      renderServices();
      updateStats();
    }
  } catch (err) {
    console.error('Failed to load services:', err);
  }
}

function updateStats() {
  const total = allServices.length;
  const running = allServices.filter(s => s.runtime.status === 'running').length;
  const stopped = allServices.filter(s => s.runtime.status === 'stopped').length;
  const alerts = allServices.filter(s => s.runtime.status === 'crashed' || s.runtime.status === 'degraded').length;

  document.getElementById('stat-total').textContent = total;
  document.getElementById('stat-running').textContent = running;
  document.getElementById('stat-stopped').textContent = stopped;
  document.getElementById('stat-alerts').textContent = alerts;
}

function renderServices() {
  const grid = document.getElementById('services-grid');
  const emptyState = document.getElementById('empty-state');
  const searchQuery = document.getElementById('service-search').value.toLowerCase().trim();

  let filtered = allServices.filter(s => {
    if (activeFilter !== 'all' && s.service_type !== activeFilter) {
      return false;
    }
    if (searchQuery) {
      const matchName = s.name.toLowerCase().includes(searchQuery);
      const matchId = s.id.toLowerCase().includes(searchQuery);
      const matchType = s.service_type.toLowerCase().includes(searchQuery);
      const matchPort = s.listen_port.toString().includes(searchQuery);
      return matchName || matchId || matchType || matchPort;
    }
    return true;
  });

  if (allServices.length === 0) {
    grid.innerHTML = '';
    emptyState.classList.remove('hidden');
    return;
  }

  emptyState.classList.add('hidden');
  grid.innerHTML = filtered.map(s => renderServiceCard(s)).join('');
}

function renderServiceCard(s) {
  const isRunning = s.runtime.status === 'running';
  const statusClass = s.runtime.status;
  const statusLabel = s.runtime.status.replace('_', ' ');

  // Latency display
  let latencyHtml = '<span class="text-muted">-</span>';
  if (s.runtime.latency_ms !== null && s.runtime.latency_ms !== undefined) {
    const ms = s.runtime.latency_ms;
    const latencyClass = ms < 100 ? 'latency-good' : ms < 300 ? 'latency-fair' : 'latency-poor';
    latencyHtml = `<span class="${latencyClass}">${ms} ms</span>`;
  }

  // Uptime display
  const uptime = s.runtime.uptime_secs ? formatDuration(s.runtime.uptime_secs) : '-';

  // Memory display
  const memory = s.runtime.memory_bytes ? (s.runtime.memory_bytes / 1024 / 1024).toFixed(1) + ' MB' : '-';

  return `
    <div class="service-card" id="card-${s.id}">
      <div>
        <div class="card-header">
          <div class="card-title-group">
            <span class="driver-badge ${s.service_type}">${s.service_type}</span>
            <div>
              <div class="card-name">${escapeHtml(s.name)}</div>
              <div class="card-id">${escapeHtml(s.id)} &bull; ${s.listen_host}:${s.listen_port}</div>
            </div>
          </div>
          <span class="status-badge ${statusClass}">
            ${statusLabel}
          </span>
        </div>

        <div class="card-metrics">
          <div class="metric-item">
            <span class="metric-label">Latency</span>
            <div class="metric-value">
              ${latencyHtml}
              <button class="btn-icon btn-sm" title="Probe Connectivity" onclick="probeService('${s.id}')">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>
              </button>
            </div>
          </div>
          <div class="metric-item">
            <span class="metric-label">Uptime</span>
            <div class="metric-value">${uptime}</div>
          </div>
          <div class="metric-item">
            <span class="metric-label">Memory / PID</span>
            <div class="metric-value">${memory} ${s.runtime.pid ? `(${s.runtime.pid})` : ''}</div>
          </div>
          <div class="metric-item">
            <span class="metric-label">Restarts</span>
            <div class="metric-value">${(s.max_restart_retries === 0 || s.restart_policy === 'always') ? `${s.runtime.restart_count} (loop)` : `${s.runtime.restart_count}/${s.max_restart_retries != null ? s.max_restart_retries : 5}`}</div>
          </div>
        </div>
      </div>

      ${renderCardAlert(s)}

      <div class="card-actions">
        <label class="switch" title="Toggle Service Power">
          <input type="checkbox" ${isRunning ? 'checked' : ''} onchange="toggleServicePower('${s.id}', this.checked)" />
          <span class="slider"></span>
        </label>

        <div class="action-buttons">
          <button class="btn-icon" title="Restart Service" onclick="restartService('${s.id}')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>
          </button>
          <button class="btn-icon" title="View Live Logs" onclick="openLogsModal('${s.id}', '${s.name}', '${s.service_type}')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>
          </button>
          <button class="btn-icon" title="Edit Configuration" onclick="openEditModal('${s.id}')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
          </button>
          <button class="btn-icon danger" title="Delete Service" onclick="deleteService('${s.id}')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
          </button>
        </div>
      </div>
    </div>
  `;
}

// Helpers for conflict & adoption parsing
function parseConflictMessage(text) {
  if (!text) return null;
  const detailedMatch = text.match(/Target port\s+([^ ]+)\s*(?:\(([^)]+)\))?\s*from configuration file is already in use by PID\s*(\d+)(?:\s*\(([^)]+)\))?(?:\s*—\s*not adopting(?:\s*\(executable does not match\s*([^)]+)\))?)?/i);
  if (detailedMatch) {
    return {
      portStr: detailedMatch[1],
      protocol: detailedMatch[2] || 'TCP',
      pid: detailedMatch[3],
      holderExe: detailedMatch[4] || '',
      expectedExe: detailedMatch[5] || '',
      raw: text
    };
  }
  const genericMatch = text.match(/(?:Address already in use|port\s*([0-9]+)\s*already in use)/i);
  if (genericMatch) {
    return {
      portStr: genericMatch[1] || 'Occupied',
      protocol: 'TCP',
      pid: null,
      holderExe: '',
      expectedExe: '',
      raw: text
    };
  }
  return null;
}

function parseAdoptedMessage(text) {
  if (!text) return null;
  const match = text.match(/Leftover process PID\s*(\d+)(?:\s*\(([^)]+)\))?\s*adopted successfully/i);
  if (match) {
    return {
      pid: match[1],
      exe: match[2] || '',
      raw: text
    };
  }
  return null;
}

function copyKillCmd(pid, btn) {
  const cmd = `kill -9 ${pid}`;
  navigator.clipboard.writeText(cmd).then(() => {
    const origHtml = btn.innerHTML;
    btn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg> Copied!`;
    btn.style.borderColor = '#10b981';
    btn.style.color = '#34d399';
    setTimeout(() => {
      btn.innerHTML = origHtml;
      btn.style.borderColor = '';
      btn.style.color = '';
    }, 2000);
  }).catch(() => {
    prompt('Copy command to kill process:', cmd);
  });
}

function dismissCardAlert(id) {
  delete serviceErrors[id];
  delete serviceAdopted[id];
  renderServices();
}

function renderCardAlert(s) {
  if (serviceErrors[s.id]) {
    const errData = serviceErrors[s.id];
    const parsed = parseConflictMessage(errData.message);
    if (parsed && parsed.pid) {
      return `
        <div class="card-alert-banner banner-error">
          <div class="banner-top">
            <span class="banner-badge">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
              Port Conflict Alert
            </span>
            <button class="banner-dismiss" onclick="dismissCardAlert('${s.id}')" title="Dismiss Alert">&times;</button>
          </div>
          <div class="conflict-meta-box" style="margin: 0.2rem 0;">
            <div class="meta-row">
              <span class="meta-lbl">Target Port</span>
              <span class="meta-val highlight-port">${parsed.portStr} (${parsed.protocol})</span>
            </div>
            <div class="meta-row">
              <span class="meta-lbl">Occupying PID</span>
              <span class="meta-val highlight-pid">${parsed.pid}</span>
            </div>
            ${parsed.holderExe ? `
            <div class="meta-row">
              <span class="meta-lbl">Active Binary</span>
              <span class="meta-val highlight-foreign">${escapeHtml(parsed.holderExe)}</span>
            </div>` : ''}
          </div>
          <div class="banner-actions">
            <button class="toast-btn toast-btn-danger" onclick="copyKillCmd('${parsed.pid}', this)">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M18 6L6 18M6 6l12 12"/></svg>
              Copy 'kill -9 ${parsed.pid}'
            </button>
            <button class="toast-btn toast-btn-outline" onclick="openEditModal('${s.id}')">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              Edit Config
            </button>
          </div>
        </div>
      `;
    } else {
      return `
        <div class="card-alert-banner banner-error">
          <div class="banner-top">
            <span class="banner-badge">Start Error</span>
            <button class="banner-dismiss" onclick="dismissCardAlert('${s.id}')" title="Dismiss Alert">&times;</button>
          </div>
          <div class="banner-body">${escapeHtml(errData.message)}</div>
        </div>
      `;
    }
  }

  if (serviceAdopted[s.id]) {
    const adoptData = serviceAdopted[s.id];
    const parsed = parseAdoptedMessage(adoptData.message);
    return `
      <div class="card-alert-banner banner-info">
        <div class="banner-top">
          <span class="banner-badge">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg>
            Process Adopted
          </span>
          <button class="banner-dismiss" onclick="dismissCardAlert('${s.id}')" title="Dismiss Alert">&times;</button>
        </div>
        ${parsed && parsed.pid ? `
          <div class="adopted-meta-box" style="margin: 0.2rem 0;">
            <div class="meta-row">
              <span class="meta-lbl">Adopted PID</span>
              <span class="meta-val highlight-pid">${parsed.pid}</span>
            </div>
            ${parsed.exe ? `
            <div class="meta-row">
              <span class="meta-lbl">Executable</span>
              <span class="meta-val" style="color: #34d399;">${escapeHtml(parsed.exe)}</span>
            </div>` : ''}
            <div class="adopted-status-note">
              Reconnected and adopted existing process without service disruption.
            </div>
          </div>
        ` : `
          <div class="banner-body">${escapeHtml(adoptData.message)}</div>
        `}
      </div>
    `;
  }
  return '';
}

// Toast Notifications
function showStylefulToast(type, title, message, serviceId = null, extra = null, duration = null) {
  const container = document.getElementById('toast-container');
  if (!container) return;

  const toast = document.createElement('div');
  toast.className = `toast toast-${type}`;

  const iconMap = {
    info: 'ℹ️',
    error: '✖',
    success: '✔',
  };
  const icon = iconMap[type] || '🔔';

  const conflict = (type === 'error') ? parseConflictMessage(message) : null;
  const adopted = (type === 'info' || (extra && extra.adopted)) ? parseAdoptedMessage(message) : null;

  let bodyHtml = '';
  if (conflict && conflict.pid) {
    bodyHtml = `
      <div class="toast-message" style="margin-bottom: 0.5rem;">Target port is blocked by an unadoptable process.</div>
      <div class="conflict-meta-box">
        <div class="meta-row">
          <span class="meta-lbl">Target Port</span>
          <span class="meta-val highlight-port">${conflict.portStr} (${conflict.protocol})</span>
        </div>
        <div class="meta-row">
          <span class="meta-lbl">Occupying PID</span>
          <span class="meta-val highlight-pid">${conflict.pid}</span>
        </div>
        ${conflict.holderExe ? `
        <div class="meta-row">
          <span class="meta-lbl">Active Binary</span>
          <span class="meta-val highlight-foreign">${escapeHtml(conflict.holderExe)}</span>
        </div>` : ''}
        ${conflict.expectedExe ? `
        <div class="meta-row">
          <span class="meta-lbl">Expected Exe</span>
          <span class="meta-val meta-muted">${escapeHtml(conflict.expectedExe)}</span>
        </div>` : ''}
      </div>
      <div class="toast-actions-bar">
        <button class="toast-btn toast-btn-danger" onclick="copyKillCmd('${conflict.pid}', this)">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M18 6L6 18M6 6l12 12"/></svg>
          Copy 'kill -9 ${conflict.pid}'
        </button>
        ${serviceId ? `
        <button class="toast-btn toast-btn-outline" onclick="openEditModal('${serviceId}')">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
          Change Port
        </button>` : ''}
      </div>
    `;
    if (duration === null) duration = 10000;
  } else if (adopted && adopted.pid) {
    bodyHtml = `
      <div class="adopted-meta-box">
        <div class="meta-row">
          <span class="meta-lbl">Adopted PID</span>
          <span class="meta-val highlight-pid">${adopted.pid}</span>
        </div>
        ${adopted.exe ? `
        <div class="meta-row">
          <span class="meta-lbl">Binary</span>
          <span class="meta-val" style="color: #34d399;">${escapeHtml(adopted.exe)}</span>
        </div>` : ''}
        <div class="adopted-status-note">
          Matching process was running on target port and has been cleanly adopted.
        </div>
      </div>
    `;
    if (duration === null) duration = 7000;
  } else {
    bodyHtml = `
      <div class="toast-message">${escapeHtml(message)}</div>
    `;
    if (duration === null) duration = type === 'error' ? 8000 : 5000;
  }

  toast.innerHTML = `
    <div class="toast-header">
      <div class="toast-title-group">
        <div class="toast-icon">${icon}</div>
        <div class="toast-title">${escapeHtml(title)}</div>
        ${serviceId ? `<span class="toast-tag">${escapeHtml(serviceId)}</span>` : ''}
      </div>
      <button class="toast-close" title="Dismiss">&times;</button>
    </div>
    <div class="toast-body">${bodyHtml}</div>
  `;

  const closeBtn = toast.querySelector('.toast-close');
  const dismiss = () => {
    toast.classList.add('toast-hiding');
    setTimeout(() => toast.remove(), 250);
  };
  closeBtn.addEventListener('click', dismiss);

  container.appendChild(toast);
  if (duration > 0) {
    setTimeout(dismiss, duration);
  }
}

// Service Actions
async function toggleServicePower(id, enable) {
  const endpoint = enable ? `/api/services/${id}/start` : `/api/services/${id}/stop`;
  try {
    const res = await fetch(endpoint, { method: 'POST' });
    const text = await res.text();
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }

    if (!res.ok) {
      const err = (data && (data.error || data.message)) || text || 'Unknown error';
      serviceErrors[id] = { message: err };
      delete serviceAdopted[id];
      showStylefulToast('error', `Start Refused: ${id}`, err, id);
    } else {
      if (data && data.adopted) {
        delete serviceErrors[id];
        serviceAdopted[id] = { message: data.message || `Adopted existing process successfully.` };
        showStylefulToast('info', `Leftover Process Adopted: ${id}`, data.message || `Adopted existing process successfully.`, id, data);
      } else {
        delete serviceErrors[id];
        delete serviceAdopted[id];
        showStylefulToast('success', `Service ${enable ? 'Started' : 'Stopped'}`, `Service '${id}' is now ${enable ? 'running' : 'stopped'}.`);
      }
    }
    loadServices();
  } catch (err) {
    console.error('Action failed', err);
    serviceErrors[id] = { message: err.message || String(err) };
    showStylefulToast('error', `Action Failed: ${id}`, err.message || String(err), id);
    loadServices();
  }
}

async function restartService(id) {
  try {
    const res = await fetch(`/api/services/${id}/restart`, { method: 'POST' });
    const text = await res.text();
    let data;
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }

    if (!res.ok) {
      const err = (data && (data.error || data.message)) || text || 'Unknown error';
      serviceErrors[id] = { message: err };
      delete serviceAdopted[id];
      showStylefulToast('error', `Restart Refused: ${id}`, err, id);
    } else {
      if (data && data.adopted) {
        delete serviceErrors[id];
        serviceAdopted[id] = { message: data.message || `Adopted existing process successfully.` };
        showStylefulToast('info', `Leftover Process Adopted: ${id}`, data.message || `Adopted existing process successfully.`, id, data);
      } else {
        delete serviceErrors[id];
        delete serviceAdopted[id];
        showStylefulToast('success', `Service Restarted`, `Service '${id}' was restarted.`);
      }
    }
    loadServices();
  } catch (err) {
    console.error('Restart failed', err);
    serviceErrors[id] = { message: err.message || String(err) };
    showStylefulToast('error', `Restart Failed: ${id}`, err.message || String(err), id);
  }
}

async function probeService(id) {
  try {
    const res = await fetch(`/api/services/${id}/probe`, { method: 'POST' });
    const data = await res.json();
    if (data.success) {
      showStylefulToast('success', `Probe Result: ${id}`, `Latency: ${data.latency_ms} ms`);
    } else {
      showStylefulToast('error', `Probe Failed: ${id}`, data.error || 'Connection probe timed out', id);
    }
  } catch (err) {
    showStylefulToast('error', `Probe Request Error: ${id}`, err.message || String(err), id);
  }
}

async function deleteService(id) {
  if (!confirm(`Are you sure you want to delete service '${id}'?`)) return;
  try {
    const res = await fetch(`/api/services/${id}`, { method: 'DELETE' });
    if (res.ok) {
      allServices = allServices.filter(s => s.id !== id);
      delete serviceErrors[id];
      delete serviceAdopted[id];
      renderServices();
      updateStats();
      showStylefulToast('success', 'Service Deleted', `Service '${id}' has been removed.`);
    }
  } catch (err) {
    console.error('Delete failed', err);
    showStylefulToast('error', 'Delete Failed', err.message || String(err));
  }
}

async function startAllServices() {
  for (const s of allServices) {
    try {
      const res = await fetch(`/api/services/${s.id}/start`, { method: 'POST' });
      const text = await res.text();
      let data;
      try { data = JSON.parse(text); } catch { data = null; }
      if (!res.ok) {
        const err = (data && (data.error || data.message)) || text || 'Unknown error';
        serviceErrors[s.id] = { message: err };
        delete serviceAdopted[s.id];
        showStylefulToast('error', `Start Refused: ${s.id}`, err, s.id);
      } else if (data && data.adopted) {
        delete serviceErrors[s.id];
        serviceAdopted[s.id] = { message: data.message };
        showStylefulToast('info', `Leftover Process Adopted: ${s.id}`, data.message, s.id, data);
      } else {
        delete serviceErrors[s.id];
      }
    } catch (e) {
      console.error(e);
    }
  }
  loadServices();
}

async function stopAllServices() {
  for (const s of allServices) {
    delete serviceErrors[s.id];
    delete serviceAdopted[s.id];
    await fetch(`/api/services/${s.id}/stop`, { method: 'POST' });
  }
  loadServices();
}

// Modals Management
function openAddModal() {
  document.getElementById('modal-title').textContent = 'Add New Service';
  document.getElementById('service-form').reset();
  document.getElementById('form-is-edit').value = 'false';
  document.getElementById('form-id').disabled = false;
  document.querySelector('input[name="driver_type"][value="overtls"]').checked = true;
  switchDriverFields('overtls');
  document.getElementById('form-max-retries').value = '0';
  document.getElementById('form-restart-delay').value = '2';
  document.getElementById('service-modal').classList.remove('hidden');
}

function openEditModal(id) {
  const service = allServices.find(s => s.id === id);
  if (!service) return;

  document.getElementById('modal-title').textContent = `Edit Service: ${service.name}`;
  document.getElementById('form-is-edit').value = 'true';

  document.getElementById('form-id').value = service.id;
  document.getElementById('form-id').disabled = true;
  document.getElementById('form-name').value = service.name;
  document.getElementById('form-listen-host').value = service.listen_host;
  document.getElementById('form-listen-port').value = service.listen_port;
  document.getElementById('form-bin-path').value = service.bin_path || '';

  const serviceType = normalizeServiceType(service.service_type);
  const typeRadio = document.querySelector(`input[name="driver_type"][value="${serviceType}"]`);
  if (typeRadio) {
    typeRadio.checked = true;
    switchDriverFields(serviceType);
  }

  const s = innerSettings(service);
  if (serviceType === 'overtls' || serviceType === 'overtls-chain') {
    document.getElementById('form-overtls-host').value = s.server_host || '';
    document.getElementById('form-overtls-port').value = s.server_port || 443;
    document.getElementById('form-overtls-password').value = s.password || '';
    document.getElementById('form-overtls-path').value = s.tunnel_path || '/secret-tunnel-path/';
    document.getElementById('form-overtls-client-id').value = s.client_id || '';
    document.getElementById('form-overtls-domain').value = s.server_domain || '';
  } else if (serviceType === 'clean-dns') {
    document.getElementById('form-dns-bind').value = s.bind || '127.0.0.1:5353';
    document.getElementById('form-dns-api').value = s.api_port || 3002;
    document.getElementById('form-dns-config').value = s.config_path || '';
  } else if (service.service_type === 'gost') {
    document.getElementById('form-gost-listen').value = s.listen_spec || '';
    document.getElementById('form-gost-forward').value = s.forward_spec || '';
  } else if (service.service_type === 'custom') {
    document.getElementById('form-custom-cmd').value = s.command || '';
    document.getElementById('form-custom-args').value = (s.args || []).join(' ');
  }

  document.getElementById('form-restart-policy').value = service.restart_policy || 'on_failure';
  document.getElementById('form-probe-type').value = service.health_check ? service.health_check.probe_type : 'socks5';
  document.getElementById('form-max-retries').value = service.max_restart_retries != null ? service.max_restart_retries : 0;
  document.getElementById('form-restart-delay').value = service.restart_backoff_secs != null ? service.restart_backoff_secs : 2;

  document.getElementById('service-modal').classList.remove('hidden');
}

function closeServiceModal() {
  document.getElementById('service-modal').classList.add('hidden');
}

function switchDriverFields(type) {
  document.querySelectorAll('.driver-fields').forEach(el => el.classList.add('hidden'));

  if (type === 'overtls' || type === 'overtls-chain') {
    document.getElementById('fields-overtls').classList.remove('hidden');
    document.getElementById('form-probe-type').value = 'socks5';
  } else if (type === 'clean-dns') {
    document.getElementById('fields-clean-dns').classList.remove('hidden');
    document.getElementById('form-probe-type').value = 'dns';
  } else if (type === 'gost') {
    document.getElementById('fields-gost').classList.remove('hidden');
    document.getElementById('form-probe-type').value = 'http';
  } else if (type === 'custom') {
    document.getElementById('fields-custom').classList.remove('hidden');
    document.getElementById('form-probe-type').value = 'tcp';
  }
}

function normalizeServiceType(type) {
  if (type === 'clean_dns') return 'clean-dns';
  if (type === 'overtls_chain') return 'overtls-chain';
  return type;
}

function innerSettings(service) {
  const raw = (service && service.settings) || {};
  if (raw.settings && typeof raw.settings === 'object') return raw.settings;
  return raw.overtls || raw.overtls_chain || raw.clean_dns || raw.gost || raw.custom || {};
}

function parseDnsBind(bind) {
  const value = (bind || '').trim();
  if (!value) return null;
  const ipv6 = value.match(/^\[([^\]]+)\]:(\d+)$/);
  if (ipv6) {
    const port = parseInt(ipv6[2], 10);
    if (port > 0 && port <= 65535) return { host: ipv6[1], port };
    return null;
  }
  const idx = value.lastIndexOf(':');
  if (idx <= 0) return null;
  const host = value.slice(0, idx).trim();
  const port = parseInt(value.slice(idx + 1), 10);
  if (!host || !port || port > 65535) return null;
  return { host, port };
}

async function handleServiceSubmit(e) {
  e.preventDefault();

  const isEdit = document.getElementById('form-is-edit').value === 'true';
  const id = document.getElementById('form-id').value.trim();
  const name = document.getElementById('form-name').value.trim();
  let listen_host = document.getElementById('form-listen-host').value.trim();
  let listen_port = parseInt(document.getElementById('form-listen-port').value, 10);
  const bin_path = document.getElementById('form-bin-path').value.trim() || null;
  const driverType = normalizeServiceType(document.querySelector('input[name="driver_type"]:checked').value);
  const restart_policy = document.getElementById('form-restart-policy').value;
  const probe_type = document.getElementById('form-probe-type').value;
  const existing = isEdit ? allServices.find(s => s.id === id) : null;
  const existingInner = existing ? innerSettings(existing) : {};

  let settings = {};
  if (driverType === 'overtls' || driverType === 'overtls-chain') {
    settings = {
      type: driverType,
      settings: {
        ...existingInner,
        remarks: name,
        server_host: document.getElementById('form-overtls-host').value.trim(),
        server_port: parseInt(document.getElementById('form-overtls-port').value, 10) || 443,
        password: document.getElementById('form-overtls-password').value,
        tunnel_path: document.getElementById('form-overtls-path').value.trim(),
        client_id: document.getElementById('form-overtls-client-id').value.trim() || null,
        server_domain: document.getElementById('form-overtls-domain').value.trim() || null,
      }
    };
  } else if (driverType === 'clean-dns') {
    const bind = document.getElementById('form-dns-bind').value.trim();
    const parsedBind = parseDnsBind(bind);
    if (parsedBind) {
      listen_host = parsedBind.host;
      listen_port = parsedBind.port;
    }
    settings = {
      type: 'clean-dns',
      settings: {
        ...existingInner,
        bind,
        api_port: parseInt(document.getElementById('form-dns-api').value, 10) || 3002,
        config_path: document.getElementById('form-dns-config').value.trim() || null,
      }
    };
  } else if (driverType === 'gost') {
    settings = {
      type: 'gost',
      settings: {
        ...existingInner,
        listen_spec: document.getElementById('form-gost-listen').value.trim() || `http://${listen_host}:${listen_port}`,
        forward_spec: document.getElementById('form-gost-forward').value.trim() || null,
      }
    };
  } else if (driverType === 'custom') {
    const rawArgs = document.getElementById('form-custom-args').value.trim();
    settings = {
      type: 'custom',
      settings: {
        ...existingInner,
        command: document.getElementById('form-custom-cmd').value.trim(),
        args: rawArgs ? rawArgs.split(/\s+/) : [],
      }
    };
  }

  const parsedMaxRetries = parseInt(document.getElementById('form-max-retries').value, 10);
  const parsedDelay = parseInt(document.getElementById('form-restart-delay').value, 10);
  const max_restart_retries = isNaN(parsedMaxRetries) ? 0 : Math.max(0, parsedMaxRetries);
  const restart_backoff_secs = isNaN(parsedDelay) ? 2 : Math.max(1, parsedDelay);

  const payload = {
    id,
    name,
    service_type: driverType,
    enabled: existing ? existing.enabled !== false : true,
    listen_host,
    listen_port,
    bin_path,
    work_dir: existing ? existing.work_dir : null,
    restart_policy,
    max_restart_retries,
    restart_backoff_secs,
    health_check: {
      enabled: probe_type !== 'none',
      check_interval_secs: existing && existing.health_check ? existing.health_check.check_interval_secs : 10,
      timeout_secs: existing && existing.health_check ? existing.health_check.timeout_secs : 3,
      consecutive_failures_threshold: existing && existing.health_check ? existing.health_check.consecutive_failures_threshold : 3,
      probe_type,
      test_target: existing && existing.health_check ? existing.health_check.test_target : null,
    },
    env_vars: existing && existing.env_vars ? existing.env_vars : {},
    settings,
    created_at: existing && existing.created_at ? existing.created_at : new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };

  const url = isEdit ? `/api/services/${id}` : '/api/services';
  const method = isEdit ? 'PUT' : 'POST';

  try {
    const res = await fetch(url, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });

    if (res.ok) {
      closeServiceModal();
      loadServices();
      showStylefulToast('success', isEdit ? 'Service Updated' : 'Service Created', `Configuration for '${id}' has been saved.`);
    } else {
      const err = await res.text();
      showStylefulToast('error', 'Save Failed', err, id);
    }
  } catch (err) {
    showStylefulToast('error', 'Request Error', err.message || String(err), id);
  }
}

// Live Logs Viewer
async function openLogsModal(id, name, type) {
  document.getElementById('logs-title').textContent = `Logs: ${name}`;
  document.getElementById('logs-service-badge').textContent = type;
  const terminal = document.getElementById('logs-terminal');
  terminal.innerHTML = '<div class="text-muted">Loading logs...</div>';
  document.getElementById('logs-modal').classList.remove('hidden');

  // 1. Fetch initial backlog
  try {
    const res = await fetch(`/api/services/${id}/logs?limit=150`);
    if (res.ok) {
      const lines = await res.json();
      terminal.innerHTML = lines.map(line => `<div>${escapeHtml(line)}</div>`).join('');
      scrollTerminalToBottom();
    }
  } catch (err) {
    console.error('Failed to load initial logs', err);
  }

  // 2. Open live WebSocket stream
  if (logSocket) {
    logSocket.close();
  }

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const wsUrl = `${protocol}//${window.location.host}/api/services/${id}/logs/ws`;
  logSocket = new WebSocket(wsUrl);

  logSocket.onmessage = (event) => {
    const div = document.createElement('div');
    div.textContent = event.data;
    terminal.appendChild(div);

    if (document.getElementById('autoscroll-chk').checked) {
      scrollTerminalToBottom();
    }
  };
}

function closeLogsModal() {
  if (logSocket) {
    logSocket.close();
    logSocket = null;
  }
  document.getElementById('logs-modal').classList.add('hidden');
}

function clearLogsViewer() {
  document.getElementById('logs-terminal').innerHTML = '';
}

function scrollTerminalToBottom() {
  const terminal = document.getElementById('logs-terminal');
  terminal.scrollTop = terminal.scrollHeight;
}

// Helpers
function formatDuration(seconds) {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function escapeHtml(text) {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}
