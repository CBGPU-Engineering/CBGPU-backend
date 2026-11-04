'use strict';

(() => {
  const adminHost = location.hostname === 'admin.cbgpu.com';
  const equipmentKind=document.documentElement.dataset.portal;
  const equipmentHost=['print','laser'].includes(equipmentKind);
  const apiPrefix = adminHost||equipmentHost ? '/portal/api' : '/api';
  const $ = (id) => document.getElementById(id);
  const state = {
    authGeneration: 0, pendingSystem: false, user: null, auditEvents: [], emailMessages: [],
    search: '', sort: '', direction: 'asc', sqlRows: [], sqlColumns: [], authenticated: false, csrf: '', panel: 'overview', databaseMode: 'table',
    tables: [], table: null, rows: [], columns: [], offset: 0, limit: 50, total: null,
    tableRequest: 0, selectedJob: null, jobTimer: null, overviewTimer: null, editing: null,
  };
  const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
  const finite = (value) => typeof value === 'number' && Number.isFinite(value);
  const display = (value) => value === null || value === undefined ? 'Unavailable' : String(value);
  const element = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const dateTime = (value) => {
    if (!value) return 'Time unavailable';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  };
  function message(id, text, error = false) {
    const node = $(id); node.textContent = text; node.classList.toggle('error', error);
  }
  function notice(text = '', error = false) {
    message('global-message', text, error); $('global-message').hidden = !text;
  }
  function busy(button, enabled, label) {
    if (enabled) { if (!button.dataset.originalLabel) button.dataset.originalLabel = button.textContent; button.disabled = true; if (label) button.textContent = label; }
    else { button.disabled = false; if (button.dataset.originalLabel) { button.textContent = button.dataset.originalLabel; delete button.dataset.originalLabel; } }
  }
  function clearSession(text = '') {
    state.authGeneration++; state.authenticated = false; state.pendingSystem = false; state.csrf = '';
    state.user = null; state.tables = []; state.table = null;
    state.rows = []; state.columns = []; state.editing = null; state.selectedJob = null; state.tableRequest++;
    state.auditEvents = []; state.emailMessages = []; retryingEmails.clear();
    clearTimeout(state.jobTimer); clearInterval(state.overviewTimer); $('password').value = '';
    $('row-editor').close(); $('portal-view').hidden = true; $('login-view').hidden = false;
    $('login-form').hidden = false;
    $('login-email').disabled = false; $('login-email').value = ''; $('password').disabled = false;
    $('login-button').disabled = false; $('login-button').textContent = 'Sign in';
    message('login-title', equipmentHost ? (equipmentKind==='print'?'3D printing sign in':'Laser cutting sign in') : adminHost ? 'Admin sign in' : 'Developer sign in'); message('login-description', 'Sign in with your existing CBGPU account.');
    window.dispatchEvent(new Event('cbgpu-terminal-logout'));
    message('login-message', text, !!text);
    message('account-name', ''); message('account-email', ''); message('header-account-label', 'Account');
    for (const id of ['health-grid', 'traffic-content', 'system-content', 'deployment-summary', 'table-result', 'sql-result', 'deployment-jobs', 'row-editor-fields', 'release-details', 'table-list', 'schema-result', 'container-list', 'backup-list', 'email-status', 'audit-result', 'emails-result', 'email-summary', 'email-detail', 'security-identity', 'account-developers', 'offsite-status', 'monitor-status']) $(id).replaceChildren();
    $('service-log-output').textContent = 'Choose a service to inspect its recent logs.'; state.sqlRows=[];state.sqlColumns=[];state.search='';state.sort='';$('row-search').value='';$('table-search').value='';
    $('deployment-log').textContent = 'No deployment selected.'; $('sql-query').value = ''; $('audit-detail').textContent = '';
    $('audit-detail-surface').hidden = true; $('email-detail-surface').hidden = true; $('email-summary').hidden = true;
    message('account-message', ''); message('recovery-kit-message', ''); message('recovery-kit-status', 'Open this page to check recovery-kit availability.'); $('recovery-kit-download').disabled = true; $('recovery-kit-reauth').hidden = true;
    $('audit-search').value = ''; $('email-search').value = ''; $('audit-outcome').replaceChildren(element('option', '', 'All outcomes')); $('email-filter').replaceChildren(element('option', '', 'All statuses'));
    for (const id of ['selected-job-meta', 'selected-job-status', 'row-editor-key', 'table-count', 'table-message', 'sql-message', 'deployments-message', 'audit-message', 'emails-message']) $(id).textContent = '';
    $('table-select').replaceChildren(element('option', '', 'No tables loaded')); $('table-select').disabled = true; notice();
  }
  async function api(path, options = {}) {
    const generation = state.authGeneration;
    const { authFailure = true, ...init } = options;
    const method = init.method || 'GET';
    const headers = new Headers(init.headers || {});
    if (init.body !== undefined) { headers.set('Content-Type', 'application/json'); init.body = JSON.stringify(init.body); }
    if (method !== 'GET' && state.csrf) headers.set('X-CSRF-Token', state.csrf);
    const response = await fetch(apiPrefix + path, { ...init, method, headers, credentials: 'same-origin', cache: 'no-store' });
    let data;
    try { data = await response.json(); } catch { data = null; }
    if (generation !== state.authGeneration) throw new Error('The sign-in session changed. Retry this action.');
    if (!response.ok) {
      if (response.status === 401 && authFailure) clearSession('Your session ended. Please sign in again.');
      const detail = data?.detail ?? data?.message ?? data?.error;
      const error = new Error(typeof detail === 'string' ? detail : detail ? JSON.stringify(detail) : `Request failed (${response.status}).`);
      error.status = response.status; throw error;
    }
    return data || {};
  }
  function identity(user) {
    state.user = user && typeof user === 'object' ? user : {};
    const email = typeof state.user.email === 'string' ? state.user.email : '';
    const name = typeof state.user.name === 'string' ? state.user.name : '';
    message('account-name', name || email || 'Developer account'); message('account-email', name ? email : '');
    message('header-account-label', name || email || 'Account');
    $('account-button').title = email || 'Account and security';
    const details = $('security-identity'); details.replaceChildren();
    for (const [label, value] of [['Name', name || 'Not provided'], ['Email', email || 'Unavailable'], ['Sign-in protection', 'CBGPU account']]) {
      const item = element('div'); item.append(element('dt', '', label), element('dd', '', value)); details.append(item);
    }
  }
  function signedIn(csrf, user) {
    if (adminHost||equipmentHost) { location.replace(equipmentHost?'/manage/manufacturing':'/admin'); return; }
    $('password').value = ''; state.pendingSystem = false; state.authenticated = true; state.csrf = csrf || ''; identity(user);
    $('login-email').value = ''; $('login-view').hidden = true; $('portal-view').hidden = false;
    notice(); showPanel('overview');
    clearInterval(state.overviewTimer);
    state.overviewTimer = setInterval(() => {
      if (state.authenticated && state.panel === 'overview' && !document.hidden) loadOverview();
    }, 30000);
  }
  async function initialize() {
    try {
      const session = await api('/session', { authFailure: false });
      if (session.authenticated === true && session.csrf_token) signedIn(session.csrf_token, session.user);
      else clearSession();
    } catch (error) { clearSession(error.message); }
  }
  $('login-form').addEventListener('submit', async (event) => {
    event.preventDefault(); const button = $('login-button'); const email = $('login-email').value.trim();
    const password = $('password').value; busy(button, true, 'Signing in…'); message('login-message', '');
    $('login-email').disabled = true; $('password').disabled = true; $('password').value = '';
    try {
      const data = await api('/login', { method: 'POST', body: { email, password }, authFailure: false });
      if(!data.authenticated||!data.csrf_token)throw new Error('Sign-in did not complete. Please try again.');
      signedIn(data.csrf_token,data.user);
    } catch (error) { message('login-message', error.message, true); }
    finally { $('login-email').disabled = false; $('password').disabled = false; busy(button, false); if (!$('login-form').hidden) $('password').focus(); }
  });
  $('logout-button').addEventListener('click', () => signOut($('logout-button')));

  function showPanel(name) {
    if (!['overview', 'database', 'deployments', 'operations', 'sql', 'terminal', 'audit', 'emails', 'security'].includes(name)) return;
    state.panel = name; notice();
    window.dispatchEvent(new CustomEvent('cbgpu-panel',{detail:name}));
    $('refresh-button').hidden=['sql','terminal','security'].includes(name);
    message('workspace-location', {overview:'Overview',database:'Database',deployments:'Deployments',operations:'Logs & services',sql:'SQL editor',terminal:'Terminal',audit:'Audit trail',emails:'Email delivery',security:'Account & security'}[name]);
    $('auto-refresh-label').hidden = name !== 'overview';
    for (const panel of document.querySelectorAll('.panel')) panel.hidden = panel.id !== 'panel-' + name;
    for (const button of document.querySelectorAll('[data-panel]')) {
      const active = button.dataset.panel === name; button.classList.toggle('active', active);
      if (active) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current');
    }
    if (name === 'overview') loadOverview();
    if (name === 'database' && !state.tables.length) loadTables();
    if (name === 'deployments') { loadDeployments(); loadRelease(); }
    if (name === 'operations') loadOperations();
    if (name === 'audit') loadAudit();
    if (name === 'emails') loadEmails();
    if (name === 'security') loadAccount();
  }
  for (const button of document.querySelectorAll('[data-panel]')) button.addEventListener('click', () => showPanel(button.dataset.panel));
  for (const button of document.querySelectorAll('[data-open-panel]')) button.addEventListener('click', (event) => { event.preventDefault(); showPanel(button.dataset.openPanel); });
  $('refresh-button').addEventListener('click', async () => {
    const button = $('refresh-button'); busy(button, true, 'Refreshing…');
    try {
      if (state.panel === 'overview') await loadOverview();
      else if (state.panel === 'database') await loadTables();
      else if (state.panel === 'operations') await loadOperations();
      else if (state.panel === 'audit') await loadAudit();
      else if (state.panel === 'emails') await loadEmails();
      else if (state.panel === 'security') await loadAccount();
      else { await loadDeployments(); await loadRelease(); }
    } finally { busy(button, false); }
  });

  function normalizeHealth(value) {
    let raw = value; let detail = '';
    if (value && typeof value === 'object') {
      raw = value.status ?? value.healthy ?? value.ok ?? value.available;
      detail = value.detail ?? value.message ?? '';
      if (typeof detail !== 'string') detail = JSON.stringify(detail);
    }
    if (raw === true || raw === 200) return { text: 'Healthy', className: 'healthy', detail };
    if (raw === false) return { text: 'Unavailable', className: 'unhealthy', detail };
    if (raw === undefined || raw === null) return { text: 'Unknown', className: '', detail };
    const text = String(raw); const lower = text.toLowerCase();
    if (['ok', 'healthy', 'running', 'active', 'up', 'online', 'ready'].includes(lower)) return { text, className: 'healthy', detail };
    if (['degraded', 'warning', 'starting', 'initializing'].includes(lower)) return { text, className: 'warning', detail };
    return { text, className: 'unhealthy', detail };
  }
  function renderHealth(health = {}) {
    const grid = $('health-grid'); grid.replaceChildren();
    for (const [key, title] of [['frontend', 'Frontend'], ['backend', 'Backend API'], ['database', 'Database'], ['controller', 'Controller'], ['materials', 'File sharing']]) {
      const status = normalizeHealth(health[key]); const card = element('article', 'health-card');
      card.append(element('h3', '', title)); const value = element('p', 'health-value');
      const dot = element('span', 'status-dot ' + status.className); dot.setAttribute('aria-hidden', 'true');
      value.append(dot, document.createTextNode(status.text)); card.append(value);
      if (status.detail) card.append(element('p', 'health-detail', status.detail));
      grid.append(card);
    }
  }
  function renderTraffic(traffic = {}) {
    const root = $('traffic-content'); root.replaceChildren();
    const since = traffic.tracking_since;
    const hasStart = since !== undefined && since !== null && since !== '' && !Number.isNaN(new Date(since).getTime());
    message('traffic-help', 'Visitors count anonymous browsers.' + (hasStart ? ' Collection started ' + dateTime(since) + '.' : ''));
    if (!traffic.available) { root.append(element('p', 'empty-state', 'Traffic data is not available. Connect a traffic data source to display visits here.')); return; }
    const totals = element('div', 'traffic-totals');
    for (const [value, label] of [[traffic.today_views, 'Views today'], [traffic.today_visitors, 'Visitors today']]) {
      const stat = element('div', 'traffic-stat'); stat.append(element('p', 'stat-number', finite(value) ? number.format(value) : 'Unavailable'), element('p', 'stat-label', label)); totals.append(stat);
    }
    root.append(totals);
    const days = Array.isArray(traffic.last_7_days) ? traffic.last_7_days.slice(-7) : [];
    if (!days.length) { root.append(element('p', 'empty-state', 'No daily traffic records are available yet.')); return; }
    const chart = element('div', 'traffic-chart'); chart.setAttribute('role', 'list'); chart.setAttribute('aria-label', 'Daily page views');
    const max = Math.max(1, ...days.map(day => finite(day.views) ? Math.max(0, day.views) : 0));
    for (const day of days) {
      const cell = element('div', 'chart-day'); cell.setAttribute('role', 'listitem');
      const value = finite(day.views) ? number.format(day.views) : 'Unavailable';
      const label = day.date ? String(day.date).slice(5, 10) : 'Unknown';
      cell.setAttribute('aria-label', `${display(day.date)}: ${value} views, ${finite(day.visitors) ? number.format(day.visitors) : 'unavailable'} visitors`);
      const track = element('div', 'chart-bar-track'); track.setAttribute('aria-hidden', 'true');
      const bar = element('div', 'chart-bar'); bar.style.height = (finite(day.views) ? Math.max(0, day.views) / max * 100 : 0) + '%'; track.append(bar);
      cell.append(element('span', 'chart-value', value), track, element('span', 'meta', label)); chart.append(cell);
    }
    root.append(chart);
  }
  function uptime(seconds) {
    if (!finite(seconds) || seconds < 0) return 'Unavailable';
    const days = Math.floor(seconds / 86400), hours = Math.floor(seconds % 86400 / 3600), minutes = Math.floor(seconds % 3600 / 60);
    return [days ? `${days}d` : '', hours ? `${hours}h` : '', `${minutes}m`].filter(Boolean).join(' ');
  }
  function renderSystem(system = {}) {
    const root = $('system-content'); root.replaceChildren(); const list = element('div', 'resource-list');
    for (const [key, label] of [['cpu_percent', 'CPU'], ['memory_percent', 'Memory'], ['disk_percent', 'Disk']]) {
      const row = element('div', 'resource-row'); const top = element('div', 'resource-top');
      top.append(element('span', '', label), element('span', 'resource-value', finite(system[key]) ? number.format(system[key]) + '%' : 'Unavailable'));
      const meter = element('div', 'meter'); meter.setAttribute('aria-hidden', 'true'); const fill = element('div', 'meter-fill'); fill.style.width = (finite(system[key]) ? Math.max(0, Math.min(100, system[key])) : 0) + '%'; meter.append(fill); row.append(top, meter); list.append(row);
    }
    const time = element('div', 'uptime-row'); time.append(element('span', '', 'Host uptime'), element('span', '', uptime(system.uptime_seconds))); root.append(list, time);
  }
  function renderDeploymentSummary(data = {}) {
    const root = $('deployment-summary'); root.replaceChildren();
    const entries = [
      ['Commit', data.commit ? String(data.commit).slice(0, 12) : 'Unavailable'], ['Branch', display(data.branch)],
      ['Working tree', data.dirty === true ? 'Uncommitted changes' : data.dirty === false ? 'Clean' : 'Unavailable'], ['Active build', display(data.active_build)],
    ];
    for (const [key, value] of entries) { const item = element('div'); item.append(element('dt', '', key), element('dd', '', value)); root.append(item); }
  }
  let overviewRequest = 0;
  async function loadOverview() {
    const request = ++overviewRequest;
    try {
      const data = await api('/overview'); if (!state.authenticated || request !== overviewRequest) return;
      renderHealth(data.health); renderTraffic(data.traffic); renderSystem(data.system); renderDeploymentSummary(data.deployment);
      message('overview-updated', data.updated_at ? 'Updated ' + dateTime(data.updated_at) : 'Update time unavailable.');
    } catch (error) { if (state.authenticated) { notice(error.message, true); message('overview-updated', 'Refresh failed.', true); } }
  }

  async function loadTables() {
    try {
      const data = await api('/tables'); if (!state.authenticated) return;
      state.tables = Array.isArray(data.tables) ? data.tables : [];
      const select = $('table-select'); const previous = state.table ? `${state.table.schema}.${state.table.name}` : '';
      select.replaceChildren();
      if (!state.tables.length) { select.append(element('option', '', 'No tables available')); select.disabled = true; $('table-refresh').disabled = true; $('table-result').replaceChildren(); message('table-message', 'No tables are available to this connection.'); return; }
      for (const [index, table] of state.tables.entries()) { const option = element('option', '', `${table.schema}.${table.name}`); option.value = String(index); select.append(option); }
      let index = state.tables.findIndex(table => `${table.schema}.${table.name}` === previous); if (index < 0) index = 0;
      select.value = String(index); select.disabled = false; $('table-refresh').disabled = false;
      state.table = state.tables[index]; state.offset = 0; renderTableList(); await loadRows();
    } catch (error) { if (state.authenticated) { message('table-message', error.message, true); $('table-select').replaceChildren(element('option', '', 'Tables unavailable')); $('table-select').disabled = true; } }
  }
  $('table-select').addEventListener('change', () => { state.table = state.tables[Number($('table-select').value)]; state.offset = 0; state.sort = ''; state.search = ''; $('row-search').value = ''; renderTableList(); loadRows(); });
  $('table-refresh').addEventListener('click', () => loadRows());
  $('table-previous').addEventListener('click', () => { state.offset = Math.max(0, state.offset - state.limit); loadRows(); });
  $('table-next').addEventListener('click', () => { state.offset += state.limit; loadRows(); });
  function normalizeColumns(columns, rows, metadata = []) {
    const originals = new Map(metadata.map(column => [column.name, column]));
    const values = Array.isArray(columns) && columns.length ? columns : rows[0] && !Array.isArray(rows[0]) ? Object.keys(rows[0]) : [];
    return values.map(column => typeof column === 'string' ? { ...originals.get(column), name: column } : { ...originals.get(column.name), ...column, type: column.type || column.data_type || originals.get(column.name)?.type });
  }
  function cellText(value) {
    if (value === null || value === undefined) return 'NULL';
    return typeof value === 'object' ? JSON.stringify(value) : String(value);
  }
  function dataTable(columns, rows, editable = false) {
    const table = element('table', 'data-table'); const head = element('thead'); const heading = element('tr');
    for (const column of columns) {
      const th = element('th'); th.scope = 'col';
      if (editable) { const sort = element('button','sort-button',column.name + (state.sort === column.name ? state.direction === 'asc' ? ' ↑' : ' ↓' : '')); sort.type='button'; sort.setAttribute('aria-label','Sort by '+column.name); sort.addEventListener('click',()=>{state.direction=state.sort===column.name&&state.direction==='asc'?'desc':'asc';state.sort=column.name;state.offset=0;loadRows()}); th.append(sort); } else th.textContent=column.name;
      if (column.type) th.append(element('span', 'column-type', column.type)); heading.append(th);
    }
    if (editable) { const th = element('th', '', 'Actions'); th.scope = 'col'; heading.append(th); }
    head.append(heading); table.append(head); const body = element('tbody');
    for (const [index, row] of rows.entries()) {
      const tr = element('tr');
      for (const [columnIndex, column] of columns.entries()) {
        const value = Array.isArray(row) ? row[columnIndex] : row[column.name]; const text = cellText(value);
        const td = element('td', value === null || value === undefined ? 'null-value' : '', text); td.title = text; tr.append(td);
      }
      if (editable) { const td = element('td'); const button = element('button', 'text-button', 'Edit'); button.type = 'button'; button.setAttribute('aria-label', `Edit row ${state.offset + index + 1}`); button.addEventListener('click', () => editRow(row)); td.append(button); tr.append(td); }
      body.append(tr);
    }
    table.append(body); return table;
  }
  async function loadRows() {
    if (!state.table) return;
    $('table-export').disabled=true;
    const request = ++state.tableRequest; const table = state.table; const offset = state.offset;
    message('table-message', 'Loading rows…'); $('table-previous').disabled = true; $('table-next').disabled = true; $('table-refresh').disabled = true;
    try {
      const params = new URLSearchParams({ schema: table.schema, name: table.name, limit: String(state.limit), offset: String(offset), search: state.search, direction: state.direction, ...(state.sort ? {sort:state.sort} : {}) });
      const data = await api('/table?' + params); if (!state.authenticated || request !== state.tableRequest) return;
      state.rows = Array.isArray(data.rows) ? data.rows : []; state.total = finite(data.total) ? data.total : null;
      state.columns = normalizeColumns(data.columns, state.rows, table.columns || []);
      const keys = state.columns.filter(column => column.primary_key);
      const editable = keys.length > 0 && state.rows.every(row => !Array.isArray(row));
      const result = $('table-result'); result.replaceChildren();
      if (!state.rows.length) { message('table-message', offset ? 'No more rows on this page.' : 'This table has no rows.'); }
      else { result.append(dataTable(state.columns, state.rows, editable)); message('table-message', editable ? 'Use Edit to change a row. Primary keys identify the row and remain unchanged.' : 'Rows are read-only here because no primary key is available. Use the SQL editor for other changes.'); }
      $('table-export').disabled = !state.rows.length; renderSchema();
      const last = offset + state.rows.length;
      message('table-count', state.rows.length ? `Rows ${number.format(offset + 1)}–${number.format(last)}${state.total !== null ? ` of ${number.format(state.total)}` : ''}` : state.total === 0 ? '0 rows' : 'No rows displayed');
      $('table-previous').disabled = offset === 0; $('table-next').disabled = state.total !== null ? last >= state.total : state.rows.length < state.limit;
    } catch (error) { if (state.authenticated && request === state.tableRequest) { message('table-message', error.message, true); $('table-result').replaceChildren(); message('table-count', ''); } }
    finally { if (request === state.tableRequest) $('table-refresh').disabled = false; }
  }
  function serialized(value) { return value === null || value === undefined ? '' : typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value); }
  function editRow(row) {
    state.editing = { row, table: state.table, columns: state.columns };
    const keys = state.columns.filter(column => column.primary_key); const key = Object.fromEntries(keys.map(column => [column.name, row[column.name]]));
    message('row-editor-table', `${state.table.schema}.${state.table.name}`); message('row-editor-key', JSON.stringify(key)); message('row-editor-message', 'Only changed fields will be updated.');
    const fields = $('row-editor-fields'); fields.replaceChildren();
    for (const [index, column] of state.columns.entries()) {
      const field = element('div', 'editor-field'); const labelWrap = element('div'); const label = element('label', '', column.name); label.htmlFor = 'edit-field-' + index;
      labelWrap.append(label, element('span', 'meta', [column.type, column.primary_key ? 'Primary key' : ''].filter(Boolean).join(' · ')));
      const value = element('div', 'editor-value'); const input = element('textarea'); input.id = 'edit-field-' + index; input.rows = typeof row[column.name] === 'object' && row[column.name] !== null ? 4 : 1; input.value = serialized(row[column.name]); input.spellcheck = false; input.dataset.column = column.name; input.readOnly = !!column.primary_key;
      const isNull = row[column.name] === null; input.disabled = isNull;
      value.append(input);
      if (column.nullable && !column.primary_key) {
        const nullLabel = element('label', 'null-checkbox'); const checkbox = element('input'); checkbox.type = 'checkbox'; checkbox.checked = isNull; checkbox.dataset.nullFor = column.name;
        checkbox.addEventListener('change', () => { input.disabled = checkbox.checked; }); nullLabel.append(checkbox, document.createTextNode('Set to NULL')); value.append(nullLabel);
      }
      field.append(labelWrap, value); fields.append(field);
    }
    $('row-editor').showModal();
  }
  for (const id of ['row-editor-close', 'row-editor-cancel']) $(id).addEventListener('click', () => $('row-editor').close());
  function editedValue(raw, original, column) {
    const type = String(column.type || '').toLowerCase();
    if (type.includes('json') || original !== null && typeof original === 'object') return JSON.parse(raw);
    if (type === 'boolean' || type === 'bool' || typeof original === 'boolean') {
      if (!['true', 'false'].includes(raw.trim().toLowerCase())) throw new Error(`${column.name} must be true or false.`);
      return raw.trim().toLowerCase() === 'true';
    }
    if (/^(smallint|integer|bigint|int2|int4|int8|numeric|decimal|real|double precision|float4|float8)$/.test(type) || typeof original === 'number') {
      if (!raw.trim() || !Number.isFinite(Number(raw))) throw new Error(`${column.name} must be a number.`);
      const value = Number(raw); return Number.isInteger(value) && !Number.isSafeInteger(value) ? raw.trim() : value;
    }
    return raw;
  }
  $('row-editor-form').addEventListener('submit', async event => {
    event.preventDefault(); if (!state.editing) return;
    const { row, table, columns } = state.editing; const values = {}; const key = {};
    try {
      for (const [index, column] of columns.entries()) {
        if (column.primary_key) { key[column.name] = row[column.name]; continue; }
        const input = $('edit-field-' + index); const checkbox = [...$('row-editor-fields').querySelectorAll('[data-null-for]')].find(node => node.dataset.nullFor === column.name);
        const isNull = checkbox ? checkbox.checked : row[column.name] === null && input.disabled;
        if (isNull && row[column.name] === null || !isNull && row[column.name] !== null && input.value === serialized(row[column.name])) continue;
        values[column.name] = isNull ? null : editedValue(input.value, row[column.name], column);
      }
      if (!Object.keys(values).length) { message('row-editor-message', 'There are no changes to save.'); return; }
      if (!window.confirm(`Update ${table.schema}.${table.name}?\n\nRow: ${JSON.stringify(key)}\nChanged columns: ${Object.keys(values).join(', ')}`)) return;
      busy($('row-editor-save'), true, 'Saving…');
      await api('/table/update', { method: 'POST', body: { schema: table.schema, table: table.name, primary_key: key, values } });
      if (!state.authenticated) return;
      $('row-editor').close(); notice('Row updated.'); await loadRows();
    } catch (error) { message('row-editor-message', error.message, true); }
    finally { busy($('row-editor-save'), false); }
  });
  function looksReadOnly(query) {
    const text = query.trim().replace(/;\s*$/, '');
    return /^(select|show)\b/i.test(text) && !/;|\b(insert|update|delete|alter|drop|create|truncate|grant|revoke|call|do|copy|execute|refresh)\b/i.test(text);
  }
  $('sql-form').addEventListener('submit', async event => {
    event.preventDefault(); const query = $('sql-query').value.trim();
    if (!query) { message('sql-message', 'Enter a SQL statement first.', true); $('sql-query').focus(); return; }
    if (query.length > 20000) { message('sql-message', 'The query must be 20,000 characters or fewer.', true); return; }
    if (!looksReadOnly(query) && !window.confirm('Run this SQL statement? It may change database data or structure.')) return;
    busy($('sql-run'), true, 'Running…'); message('sql-message', 'Running query…'); $('sql-result').replaceChildren();
    try {
      const data = await api('/sql', { method: 'POST', body: { query } }); if (!state.authenticated) return; const rows = Array.isArray(data.rows) ? data.rows : []; const columns = normalizeColumns(data.columns, rows);
      state.sqlRows=rows;state.sqlColumns=columns;$('sql-export').disabled=!rows.length;
      if (columns.length) $('sql-result').append(dataTable(columns, rows));
      const count = finite(data.row_count) ? data.row_count : rows.length;
      message('sql-message', `${number.format(count)} ${count === 1 ? 'row' : 'rows'}${finite(data.duration_ms) ? ` · ${number.format(data.duration_ms)} ms` : ''}${rows.length === 500 ? ' · Result limit reached' : ''}`);
    } catch (error) { if (state.authenticated) message('sql-message', error.message, true); }
    finally { busy($('sql-run'), false); }
  });
  $('sql-query').addEventListener('keydown', event => { if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') { event.preventDefault(); if (!$('sql-run').disabled) $('sql-form').requestSubmit(); } });

  function jobStatusClass(status) {
    if (/fail|error|cancel/i.test(String(status))) return 'failed';
    if (/success|complete|deploy|rollback|rolled_back/i.test(String(status)) && !/deploying/i.test(String(status))) return 'success';
    return '';
  }
  async function loadDeployments() {
    try {
      const data = await api('/deployments'); if (!state.authenticated) return;
      const jobs = Array.isArray(data.jobs) ? data.jobs : []; const root = $('deployment-jobs'); root.replaceChildren();
      message('deployments-message', jobs.length ? `${jobs.length} ${jobs.length === 1 ? 'deployment' : 'deployments'}` : 'No deployment jobs yet. Build a release to get started.');
      for (const job of jobs) {
        const button = element('button', 'job-button' + (String(job.id) === String(state.selectedJob) ? ' selected' : '')); button.type = 'button'; button.dataset.jobId = String(job.id);
        const top = element('div', 'job-top'); top.append(element('span', 'job-id', job.commit ? String(job.commit).slice(0, 10) : String(job.id)), element('span', 'job-status ' + jobStatusClass(job.status), display(job.status)));
        button.append(top, element('p', 'job-time', (job.action || 'deployment') + ' · ' + dateTime(job.started_at))); if(job.build_dir) button.append(element('p','job-time',job.build_dir)); button.addEventListener('click', () => selectJob(job.id)); root.append(button);
      }
      if (state.selectedJob) await loadJob(state.selectedJob);
    } catch (error) { if (state.authenticated) message('deployments-message', error.message, true); }
  }
  $('deployments-refresh').addEventListener('click', () => loadDeployments());
  function selectJob(id) {
    clearTimeout(state.jobTimer); state.selectedJob = id;
    for (const button of $('deployment-jobs').querySelectorAll('button')) button.classList.remove('selected');
    message('selected-job-meta', 'Loading deployment…'); $('deployment-log').textContent = '';
    loadDeployments();
  }
  async function loadJob(id) {
    clearTimeout(state.jobTimer);
    try {
      const data = await api('/deployments/' + encodeURIComponent(id)); if (!state.authenticated || String(state.selectedJob) !== String(id)) return;
      const job = data.job || {}; message('selected-job-status', display(job.status));
      for (const button of $('deployment-jobs').querySelectorAll('[data-job-id]')) {
        if (button.dataset.jobId !== String(id)) continue;
        const badge = button.querySelector('.job-status'); badge.textContent = display(job.status); badge.className = 'job-status ' + jobStatusClass(job.status);
      }
      message('selected-job-meta', `${job.commit ? String(job.commit).slice(0, 12) + ' · ' : ''}${dateTime(job.started_at)}${job.finished_at ? ' — ' + dateTime(job.finished_at) : ''}${job.error ? ' · ' + job.error : ''}`, !!job.error);
      const log = Array.isArray(data.log) ? data.log.join('\n') : data.log;
      $('deployment-log').textContent = log ? String(log) : 'No log output yet.';
      for(const button of document.querySelectorAll('[data-deploy-action]')) button.disabled=job.status==='running';
      const finished = job.finished_at || /^(success|succeeded|completed|complete|failed|error|cancelled|canceled|deployed|rolled_back)$/.test(String(job.status).toLowerCase());
      if (!finished) state.jobTimer = setTimeout(() => { if (state.authenticated) loadJob(id); }, 2500);
    } catch (error) { if (state.authenticated) message('selected-job-meta', error.message, true); }
  }
  function renderTableList() {
    const search=$('table-search').value.toLowerCase(); const root=$('table-list');root.replaceChildren();
    const matches=state.tables.filter(t=>(t.schema+'.'+t.name).toLowerCase().includes(search));
    message('table-list-count',matches.length+' tables');
    for(const t of matches){const button=element('button','table-link'+(state.table?.schema===t.schema&&state.table?.name===t.name?' active':''));button.type='button';button.append(element('span','',t.name),element('small','',t.schema==='public'?'':t.schema));button.title=t.schema+'.'+t.name;button.addEventListener('click',()=>{state.table=t;state.offset=0;state.sort='';state.search='';$('row-search').value='';$('table-select').value=String(state.tables.indexOf(t));renderTableList();loadRows()});root.append(button)}
    if(!matches.length)root.append(element('p','empty-state','No matching tables.'));
  }
  $('table-search').addEventListener('input',renderTableList);
  $('row-filter-form').addEventListener('submit',e=>{e.preventDefault();state.search=$('row-search').value;state.offset=0;loadRows()});
  $('row-filter-clear').addEventListener('click',()=>{state.search='';state.offset=0;$('row-search').value='';loadRows()});
  function renderSchema(){const root=$('schema-result');root.replaceChildren();if(!state.columns.length)return;root.append(dataTable([{name:'Column'},{name:'Type'},{name:'Nullable'},{name:'Primary key'}],state.columns.map(c=>({'Column':c.name,'Type':c.type,'Nullable':c.nullable?'Yes':'No','Primary key':c.primary_key?'Yes':''}))))}
  $('table-schema').addEventListener('click',()=>{$('schema-result').hidden=!$('schema-result').hidden;$('table-schema').setAttribute('aria-pressed',String(!$('schema-result').hidden))});
  function exportCsv(columns,rows,name){
    const quote=value=>{let text=cellText(value);if(/^[=+@\-\t\r]/.test(text))text="'"+text;return '"'+text.replaceAll('"','""')+'"'};
    const text=[columns.map(c=>quote(c.name)).join(','),...rows.map(row=>columns.map((c,i)=>quote(Array.isArray(row)?row[i]:row[c.name])).join(','))].join('\r\n');
    const url=URL.createObjectURL(new Blob(['\uFEFF'+text],{type:'text/csv;charset=utf-8'}));const link=element('a');link.href=url;link.download=name+'.csv';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
  }
  $('table-export').addEventListener('click',()=>exportCsv(state.columns,state.rows,state.table?.name||'table'));
  $('sql-export').addEventListener('click',()=>exportCsv(state.sqlColumns,state.sqlRows,'query-results'));
  const snippets={tables:"SELECT schemaname, relname AS table_name, n_live_tup AS estimated_rows\nFROM pg_stat_user_tables\nORDER BY n_live_tup DESC;",policies:"SELECT schemaname, tablename, policyname, roles, cmd\nFROM pg_policies\nWHERE schemaname = 'public'\nORDER BY tablename, policyname;",size:"SELECT pg_size_pretty(pg_database_size(current_database())) AS database_size;",connections:"SELECT state, count(*) AS connections\nFROM pg_stat_activity\nGROUP BY state\nORDER BY connections DESC;"};
  $('sql-snippet').addEventListener('change',()=>{if(snippets[$('sql-snippet').value]){if($('sql-query').value.trim()&&!window.confirm('Replace the current query?'))return;$('sql-query').value=snippets[$('sql-snippet').value];$('sql-query').focus()}});

  async function loadRelease(){try{const data=await api('/overview');if(!state.authenticated)return;const root=$('release-details');root.replaceChildren();for(const [key,value] of [['Domain','cbgpu.com'],['Commit',data.deployment?.commit?.slice(0,12)||'Unavailable'],['Active build',data.deployment?.active_build||'Unavailable'],['Source',data.deployment?.dirty?'Includes local changes':'Clean checkout']]){const item=element('div');item.append(element('dt','',key),element('dd','',value));root.append(item)}}catch(e){if(state.authenticated)notice(e.message,true)}}
  async function loadOperations(){
    try{const data=await api('/operations');if(!state.authenticated)return;
      message('operations-updated','Updated '+dateTime(data.updated_at));message('container-count',data.containers.length+' running');const root=$('container-list');root.replaceChildren();
      for(const c of data.containers){const row=element('div','service-row'),name=element('div');name.append(element('strong','',c.name),element('small','',c.image));row.append(name,element('span','service-status'+(/unhealthy|starting/i.test(c.status)?' warning':''),c.status));root.append(row)}
      if(!data.containers.length)root.append(element('p','empty-state','No service data available.'));
      message('backup-timer',data.backup_timer_active?'Daily timer enabled':'Timer inactive');const backups=$('backup-list');backups.replaceChildren();
      for(const b of data.backups){const row=element('div','backup-row'),name=element('div');name.append(element('span','',dateTime(b.created_at)),element('small','',(b.database?'Database':'Missing database')+' · '+(b.storage?'Storage':'Missing storage')));row.append(name,element('span','meta',number.format(b.bytes/1024/1024)+' MB'));backups.append(row)}
      if(!data.backups.length)backups.append(element('p','empty-state','No saved backups found.'));
      const email=$('email-status');email.replaceChildren();for(const [key,value] of [['Active sender',data.email?.active||'Unavailable'],['Delivery status',data.email?.status?.replaceAll('_',' ')||'Unavailable']]){const item=element('div');item.append(element('dt','',key),element('dd','',value));email.append(item)}
      renderOperationalStatus('offsite', data.offsite ?? data.offsite_backup); renderOperationalStatus('monitor', data.monitoring ?? data.monitor);
      await loadLogs();
    }catch(e){if(state.authenticated)notice(e.message,true)}
  }
  let logRequest=0;
  async function loadLogs(){const request=++logRequest;const source=$('log-source').value;message('logs-message','Loading recent logs…');$('service-log-output').textContent='';busy($('logs-refresh'),true,'Loading…');try{const data=await api('/logs?source='+encodeURIComponent(source));if(!state.authenticated||request!==logRequest)return;$('service-log-output').textContent=data.log||'No recent log entries.';message('logs-message','Last 150 entries · '+dateTime(data.updated_at))}catch(e){if(state.authenticated&&request===logRequest)message('logs-message',e.message,true)}finally{if(request===logRequest)busy($('logs-refresh'),false)}}
  $('logs-refresh').addEventListener('click',loadLogs);$('log-source').addEventListener('change',loadLogs);

  async function loadAccount() {
    busy($('account-refresh'), true, 'Loading…'); message('account-message', 'Loading account security…');
    $('recovery-kit-download').disabled = true;
    try {
      const data = await api('/account'); if (!state.authenticated) return; identity(data.user || state.user);
      $('recovery-kit-download').disabled = data.recovery_kit_available !== true;
      message('recovery-kit-status', data.recovery_kit_available === true ? 'A recovery kit is available for this account. Download and save a private copy.' : data.recovery_kit_available === false ? 'No recovery kit is available for this account.' : 'Recovery-kit availability was not returned.');
      const developers = $('account-developers'); developers.replaceChildren();
      for (const developer of Array.isArray(data.developers) ? data.developers : []) {
        if (typeof developer.email === 'string') developers.append(element('p', '', developer.email));
      }
      if (!developers.childElementCount) developers.append(element('p', 'meta', 'No developer account records are available.'));
      message('account-message', 'Account security is up to date.');
    } catch (error) { if (state.authenticated) { message('account-message', error.message, true); message('recovery-kit-status', 'Unable to check recovery-kit availability.'); } }
    finally { busy($('account-refresh'), false); }
  }
  $('account-refresh').addEventListener('click', loadAccount);
  $('recovery-kit-download').addEventListener('click', async () => {
    const button = $('recovery-kit-download'); busy(button, true, 'Downloading…'); message('recovery-kit-message', ''); $('recovery-kit-reauth').hidden = true;
    const generation = state.authGeneration;
    try {
      // Keep the attachment opaque: never parse or render recovery-kit secrets.
      const response = await fetch('/api/recovery-kit', { credentials: 'same-origin', cache: 'no-store' });
      if (generation !== state.authGeneration) return;
      if (!response.ok) {
        if (response.status === 401) { clearSession('Your session ended. Sign in again to download your recovery kit.'); return; }
        if (response.status === 403) {
          message('recovery-kit-message', 'A recent sign-in is required. Sign out and sign in again, then retry this download.', true); $('recovery-kit-reauth').hidden = false; return;
        }
        let data; try { data = await response.json(); } catch { data = null; }
        const detail = data?.detail ?? data?.message ?? data?.error;
        throw new Error(typeof detail === 'string' ? detail : `Recovery-kit download failed (${response.status}).`);
      }
      if (!(response.headers.get('Content-Type') || '').toLowerCase().includes('application/json')) throw new Error('The server did not return a JSON recovery kit.');
      const blob = await response.blob(); if (generation !== state.authGeneration) return;
      const disposition = response.headers.get('Content-Disposition') || '';
      const match = /filename=(?:"([^"]+)"|([^;]+))/i.exec(disposition);
      const name = match ? (match[1] || match[2]).trim().split(/[\\/]/).pop().replace(/[^a-zA-Z0-9._-]/g, '_') : '';
      const url = URL.createObjectURL(blob); const link = element('a'); link.href = url; link.download = name || 'cbgpu-developer-recovery-kit.json'; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
      message('recovery-kit-message', 'Recovery-kit download requested. Save the JSON file privately.');
    } catch (error) { if (state.authenticated && generation === state.authGeneration) message('recovery-kit-message', error.message, true); }
    finally { busy(button, false); if (!state.authenticated) button.disabled = true; }
  });
  $('recovery-kit-reauth').addEventListener('click', () => signOut($('recovery-kit-reauth')));

  function valuesText(value) {
    if (value === null || value === undefined || value === '') return 'Not recorded';
    return typeof value === 'object' ? JSON.stringify(value) : String(value);
  }
  function resultTable(headings) {
    const table = element('table', 'data-table'); const head = element('thead'); const row = element('tr');
    for (const heading of headings) { const th = element('th', '', heading); th.scope = 'col'; row.append(th); }
    head.append(row); table.append(head); const body = element('tbody'); table.append(body); return { table, body };
  }
  function plainCell(value, className = '') {
    const text = valuesText(value); const cell = element('td', className, text); cell.title = text; return cell;
  }
  function statusBadge(value) {
    const status = valuesText(value); const lower = status.toLowerCase();
    const type = /fail|error|denied|reject/.test(lower) ? 'failed' : /success|sent|delivered|allowed/.test(lower) ? 'success' : '';
    return element('span', 'job-status ' + type, status);
  }
  function populateFilter(id, values, allLabel) {
    const select = $(id); const previous = select.value; select.replaceChildren();
    const all = element('option', '', allLabel); all.value = ''; select.append(all);
    for (const value of [...new Set(values.filter(value => typeof value === 'string' && value))].sort()) {
      const option = element('option', '', value); option.value = value; select.append(option);
    }
    select.value = [...select.options].some(option => option.value === previous) ? previous : '';
  }
  function renderAudit() {
    const search = $('audit-search').value.trim().toLowerCase(); const outcome = $('audit-outcome').value;
    const rows = state.auditEvents.filter(event => (!outcome || event.outcome === outcome) && (!search || [event.actor_email, event.action, event.target, event.outcome, event.detail].map(valuesText).join(' ').toLowerCase().includes(search)));
    const root = $('audit-result'); root.replaceChildren();
    message('audit-message', state.auditEvents.length ? `${number.format(rows.length)} of ${number.format(state.auditEvents.length)} loaded events${state.auditEvents.length === 100 ? ' · Latest 100 requested' : ''}` : 'No audit events have been recorded yet.');
    if (!rows.length) { if (state.auditEvents.length) message('audit-message', 'No loaded events match these filters.'); return; }
    const { table, body } = resultTable(['Time', 'Developer', 'Action', 'Target', 'Outcome', 'Details']);
    for (const event of rows) {
      const row = element('tr'); row.append(plainCell(dateTime(event.at)), plainCell(event.actor_email), plainCell(event.action), plainCell(event.target));
      const outcomeCell = element('td'); outcomeCell.append(statusBadge(event.outcome)); row.append(outcomeCell);
      const action = element('td'); const view = element('button', 'text-button', 'View'); view.type = 'button'; view.setAttribute('aria-label', 'View audit event details');
      view.addEventListener('click', () => { $('audit-detail').textContent = JSON.stringify(event, null, 2); $('audit-detail-surface').hidden = false; $('audit-detail').focus(); });
      action.append(view); row.append(action); body.append(row);
    }
    root.append(table);
  }
  async function loadAudit() {
    message('audit-message', 'Loading audit events…'); busy($('audit-refresh'), true, 'Loading…');
    try {
      const data = await api('/audit?limit=100'); if (!state.authenticated) return;
      state.auditEvents = Array.isArray(data.events) ? data.events : [];
      populateFilter('audit-outcome', state.auditEvents.map(event => event.outcome), 'All outcomes'); renderAudit();
    } catch (error) { if (state.authenticated) { $('audit-result').replaceChildren(); message('audit-message', error.message, true); } }
    finally { busy($('audit-refresh'), false); }
  }
  $('audit-refresh').addEventListener('click', loadAudit); $('audit-search').addEventListener('input', renderAudit); $('audit-outcome').addEventListener('change', renderAudit);
  $('audit-detail-close').addEventListener('click', () => { $('audit-detail').textContent = ''; $('audit-detail-surface').hidden = true; });

  const retryingEmails = new Set();
  function renderEmailSummary(summary) {
    const root = $('email-summary'); root.replaceChildren();
    if (summary && typeof summary === 'object' && !Array.isArray(summary)) {
      for (const [key, value] of Object.entries(summary)) {
        if (!finite(value)) continue;
        const item = element('div'); const label = key.replaceAll('_', ' ');
        item.append(element('dt', '', label.charAt(0).toUpperCase() + label.slice(1)), element('dd', '', number.format(value))); root.append(item);
      }
    }
    root.hidden = !root.childElementCount;
  }
  function showEmailDetails(email) {
    const root = $('email-detail'); root.replaceChildren();
    for (const [label, value] of [['Message ID', email.id], ['Recipient', email.recipient], ['Subject', email.subject], ['Status', email.status], ['Attempts', email.attempts], ['Created', email.created_at ? dateTime(email.created_at) : null], ['Next attempt', email.next_attempt_at ? dateTime(email.next_attempt_at) : null], ['Provider ID', email.provider_id], ['Last error', email.last_error]]) {
      const item = element('div'); item.append(element('dt', '', label), element('dd', '', valuesText(value))); root.append(item);
    }
    $('email-detail-surface').hidden = false; root.tabIndex = -1; root.focus();
  }
  function renderEmails() {
    const search = $('email-search').value.trim().toLowerCase(); const status = $('email-filter').value;
    const rows = state.emailMessages.filter(email => (!status || email.status === status) && (!search || [email.recipient, email.subject, email.provider_id, email.last_error].map(valuesText).join(' ').toLowerCase().includes(search)));
    const root = $('emails-result'); root.replaceChildren();
    message('emails-message', state.emailMessages.length ? `${number.format(rows.length)} of ${number.format(state.emailMessages.length)} loaded messages` : 'No email messages are available.');
    if (!rows.length) { if (state.emailMessages.length) message('emails-message', 'No loaded messages match these filters.'); return; }
    const { table, body } = resultTable(['Created', 'Recipient', 'Subject', 'Status', 'Attempts', 'Next attempt', 'Actions']);
    for (const email of rows) {
      const row = element('tr'); row.append(plainCell(dateTime(email.created_at)), plainCell(email.recipient), plainCell(email.subject));
      const statusCell = element('td'); statusCell.append(statusBadge(email.status)); row.append(statusCell, plainCell(email.attempts), plainCell(email.next_attempt_at ? dateTime(email.next_attempt_at) : null));
      const actions = element('td', 'message-actions'); const view = element('button', 'text-button', 'Details'); view.type = 'button'; view.addEventListener('click', () => showEmailDetails(email)); actions.append(view);
      if (email.can_retry !== false && ['failed', 'deferred'].includes(String(email.status).toLowerCase())) {
        const retry = element('button', 'text-button', 'Retry'); retry.type = 'button'; retry.disabled = retryingEmails.has(email.id); retry.setAttribute('aria-label', 'Retry email delivery to ' + valuesText(email.recipient));
        retry.addEventListener('click', async () => {
          if (retryingEmails.has(email.id) || !window.confirm(`Retry email delivery?\n\nRecipient: ${valuesText(email.recipient)}\nSubject: ${valuesText(email.subject)}`)) return;
          retryingEmails.add(email.id); busy(retry, true, 'Requesting…');
          try { await api('/emails/retry', { method: 'POST', body: { id: email.id } }); if (!state.authenticated) return; notice('Email retry requested. Delivery status will update when the queue processes it.'); await loadEmails(); }
          catch (error) { if (state.authenticated) message('emails-message', error.message, true); }
          finally { retryingEmails.delete(email.id); busy(retry, false); if (state.authenticated) renderEmails(); }
        }); actions.append(retry);
      }
      row.append(actions); body.append(row);
    }
    root.append(table);
  }
  async function loadEmails() {
    message('emails-message', 'Loading email messages…'); busy($('emails-refresh'), true, 'Loading…');
    try {
      const data = await api('/emails'); if (!state.authenticated) return;
      state.emailMessages = Array.isArray(data.messages) ? data.messages : [];
      renderEmailSummary(data.summary); populateFilter('email-filter', state.emailMessages.map(email => email.status), 'All statuses'); renderEmails();
    } catch (error) { if (state.authenticated) { $('emails-result').replaceChildren(); message('emails-message', error.message, true); } }
    finally { busy($('emails-refresh'), false); }
  }
  $('emails-refresh').addEventListener('click', loadEmails); $('email-search').addEventListener('input', renderEmails); $('email-filter').addEventListener('change', renderEmails);
  $('email-detail-close').addEventListener('click', () => { $('email-detail').replaceChildren(); $('email-detail-surface').hidden = true; });
  function renderOperationalStatus(name, data) {
    const surface = $(name + '-surface'); const root = $(name + '-status'); root.replaceChildren();
    if (data && typeof data === 'object' && !Array.isArray(data)) {
      const labels = { status: 'Status', configured: 'Configured', enabled: 'Enabled', last_success_at: 'Last success', last_backup_at: 'Last backup', last_upload_at: 'Last upload', last_checked_at: 'Last check', message: 'Details', last_error: 'Last error' };
      for (const [key, label] of Object.entries(labels)) {
        if (!Object.hasOwn(data, key)) continue;
        const value = data[key]; const text = typeof value === 'boolean' ? value ? 'Yes' : 'No' : key.endsWith('_at') && value ? dateTime(value) : valuesText(value);
        const item = element('div'); item.append(element('dt', '', label), element('dd', '', text)); root.append(item);
      }
    }
    surface.hidden = !root.childElementCount;
  }

  initialize();
})();
