const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const state = { overview: null, system: null, business: null, rates: [], sessions: [], vouchers: [], transactions: [], sites: [], settings: null };
let revenueChart = null;
let loginVisible = false;
const pageNames = { dashboard: 'Dashboard', sales: 'Sales & inventory', sessions: 'Live sessions', rates: 'Timer rates', vouchers: 'Voucher generator', network: 'Network status', tools: 'Site blocking', settings: 'Settings' };

async function api(path, options = {}) {
  const method = (options.method || 'GET').toUpperCase();
  const headers = { 'Content-Type': 'application/json', ...(options.headers || {}) };
  if (method !== 'GET') {
    headers['X-CSRF-Token'] = localStorage.getItem('chasselfiCsrf') || '';
  }
  const response = await fetch(`/api${path}`, {
    headers,
    ...options,
  });
  const data = response.status === 204 ? null : await response.json();
  if (response.status === 401 && path !== '/login') {
    showLogin();
    const error = new Error('Login required');
    error.authRequired = true;
    throw error;
  }
  if (!response.ok) throw new Error(data?.error || `Request failed (${response.status})`);
  return data;
}

function showLogin() {
  if (loginVisible) return;
  loginVisible = true;
  document.body.classList.add('auth-required');
  $('#app').innerHTML = `<section class="auth-card card"><span class="brand-mark">C</span><span class="eyebrow">Administrator access</span><h1>Welcome back.</h1><p>Sign in to manage your vendo and customer sessions.</p><form id="login-form" class="modal-form"><div class="field"><label class="form-label">Username</label><input class="form-control" name="username" value="admin" autocomplete="username" required></div><div class="field"><label class="form-label">Password</label><input class="form-control" name="password" type="password" autocomplete="current-password" required></div><div id="login-error" class="text-danger small" hidden></div><button class="btn primary-btn w-100">Sign in</button></form><small>Set <span class="mono">CHASSELFI_ADMIN_PASSWORD</span> before deploying.</small></section>`;
  $('#login-form').onsubmit = async event => {
    event.preventDefault();
    const form = new FormData(event.target);
    const submit = event.target.querySelector('button');
    submit.disabled = true;
    try {
      const result = await api('/login', { method: 'POST', body: JSON.stringify(Object.fromEntries(form)) });
      localStorage.setItem('chasselfiCsrf', result.csrfToken);
      loginVisible = false;
      document.body.classList.remove('auth-required');
      await renderPage('dashboard');
    } catch (error) {
      $('#login-error').textContent = error.message;
      $('#login-error').hidden = false;
      submit.disabled = false;
    }
  };
}

const money = amount => new Intl.NumberFormat('en-PH', { style: 'currency', currency: 'PHP', maximumFractionDigits: 0 }).format(amount || 0);
const dateTime = value => new Intl.DateTimeFormat('en-PH', { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(value));
const duration = minutes => minutes >= 1440 ? `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h` : minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60 ? `${minutes % 60}m` : ''}` : `${minutes}m`;
const remaining = seconds => `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m`;

function toast(message, type = 'success') {
  const node = document.createElement('div');
  node.className = `toast ${type}`;
  node.textContent = message;
  $('#toasts').append(node);
  setTimeout(() => node.remove(), 3400);
}

function openModal(title, html) {
  $('#modal-title').textContent = title;
  $('#modal-body').innerHTML = html;
  bootstrap.Modal.getOrCreateInstance($('#modal')).show();
  setTimeout(() => $('#modal input, #modal select')?.focus(), 30);
}
function closeModal() { bootstrap.Modal.getOrCreateInstance($('#modal')).hide(); }

function pageHead(kicker, title, subtitle, actions = '') {
  return `<div class="page-head"><div><span class="eyebrow">${kicker}</span><h1>${title}</h1><p>${subtitle}</p></div>${actions ? `<div class="head-actions">${actions}</div>` : ''}</div>`;
}
function badge(label, type = '') { return `<span class="badge ${type}">${label}</span>`; }
function empty(title, message) { return `<div class="empty-state"><b>${title}</b>${message}</div>`; }

async function refreshCore() {
  [state.overview, state.system, state.business] = await Promise.all([api('/overview'), api('/system'), api('/business/summary')]);
}

async function renderDashboard() {
  await refreshCore();
  const o = state.overview, s = state.system, b = state.business;
  $('#app').innerHTML = `
    ${pageHead('Operations overview', 'Good day, Admin.', 'Here’s what is happening in your WiFi vendo right now.', `<button class="secondary-btn" id="refresh-dashboard">↻ Refresh data</button><a class="primary-btn" href="/portal.html" target="_blank" style="display:grid;place-items:center">Open portal ↗</a>`)}
    <section class="stats-grid">
      <article class="stat-card"><small>Sales today</small><strong>${money(o.todaySales)}</strong><footer><span class="trend-up">● Live</span> from all stations</footer></article>
      <article class="stat-card"><small>Last 7 days</small><strong>${money(o.weekSales)}</strong><footer>${o.transactionCount} recorded transactions</footer></article>
      <article class="stat-card"><small>Online users</small><strong>${o.onlineUsers}</strong><footer>${o.pausedUsers} session${o.pausedUsers === 1 ? '' : 's'} paused</footer></article>
      <article class="stat-card"><small>Ready vouchers</small><strong>${o.readyVouchers}</strong><footer>Available for redemption</footer></article>
      <article class="stat-card"><small>Average sale</small><strong>${money(b.averageTransaction)}</strong><footer>${b.uniqueClients} unique clients recorded</footer></article>
    </section>
    <div class="dashboard-grid">
      <div>
        <section class="card">
          <div class="card-head"><div><h2>Revenue pulse</h2><p>Daily sales for the last seven days</p></div>${badge(`${money(o.weekSales)} total`)}</div>
          <div class="chart-wrap"><canvas id="revenue-chart" aria-label="Seven-day revenue chart"></canvas></div>
        </section>
        <section class="card">
          <div class="card-head"><div><h2>Recent activity</h2><p>Latest coin and voucher transactions</p></div><a href="#sales" class="ghost-btn">View all</a></div>
          <div class="activity-list">${o.recentTransactions.map(tx => `<div class="activity-item"><i></i><div><p>${tx.kind} purchase · ${duration(tx.minutes)}</p><small>${tx.clientIp} · ${dateTime(tx.createdAt)}</small></div><strong>${money(tx.amount)}</strong></div>`).join('')}</div>
        </section>
      </div>
      <div>
        <section class="card">
          <div class="card-head"><div><h2>Vendo health</h2><p>Live system resources</p></div>${badge('Online')}</div>
          <div class="system-list">
            ${metric('CPU', `${s.cpuPercent}%`, Math.min(s.cpuPercent,100))}
            ${metric('Memory', `${s.memoryUsedMb} / ${s.memoryTotalMb} MB`, s.memoryTotalMb ? s.memoryUsedMb/s.memoryTotalMb*100 : 0)}
            ${metric('Uptime', formatUptime(s.uptimeSeconds), Math.min(s.uptimeSeconds / 864,100))}
            <div class="system-row"><div><span>Coin acceptor</span><strong class="${s.coinSlotOnline?'text-success':'text-warning'}">${s.coinSlotOnline?'ONLINE':'OFFLINE'}</strong></div><small>${s.coinSlotOnline?`${s.coinNodes?.length||0} network node(s) online · ${s.coinSlotMode}`:'Waiting for an authenticated network node or local pulse adapter.'}</small></div>
          </div>
        </section>
        <section class="card">
          <div class="card-head"><div><h2>Coin acceptor</h2><p>Authenticated hardware only</p></div>${badge(s.coinSlotOnline?'Ready':'Hardware required',s.coinSlotOnline?'success':'warning')}</div>
          <p class="text-secondary">ESP32, Arduino, Orange Pi, GPIO, and serial adapters report real pulses. Browser requests can never create coin credit.</p>
          <button class="btn secondary-btn w-100 mt-3" disabled>${s.coinSlotOnline?'Coin system ready':'Waiting for coin node'}</button>
        </section>
        <section class="card"><div class="card-head"><div><h2>Business snapshot</h2><p>Useful numbers for daily operations</p></div>${badge(`${money(b.readyInventoryValue)} inventory`)}</div><div class="system-list"><div class="system-row"><div><span>Coin sales</span><strong>${money(b.coinSales)}</strong></div><small>Recorded revenue</small></div><div class="system-row"><div><span>Voucher sales</span><strong>${money(b.voucherSales)}</strong></div><small>Prepaid revenue</small></div><div class="system-row"><div><span>Active sessions</span><strong>${b.activeSessions}</strong></div><small>Being enforced</small></div></div></section>
      </div>
    </div>`;
  requestAnimationFrame(() => drawRevenueChart($('#revenue-chart'), o.dailySales));
  state.rates = await api('/rates');
  $('#refresh-dashboard').onclick = () => renderPage('dashboard');
}

function metric(name, value, percent) {
  return `<div class="system-row"><div><span>${name}</span><strong>${value}</strong></div><div class="progress"><i style="width:${Math.max(2,percent)}%"></i></div></div>`;
}
function formatUptime(seconds) {
  const d = Math.floor(seconds / 86400), h = Math.floor(seconds % 86400 / 3600), m = Math.floor(seconds % 3600 / 60);
  return `${d ? `${d}d ` : ''}${h}h ${m}m`;
}
function drawRevenueChart(canvas, points) {
  if (!canvas || typeof Chart === 'undefined') return;
  if (revenueChart) revenueChart.destroy();
  const styles = getComputedStyle(document.documentElement);
  const line = styles.getPropertyValue('--line').trim();
  const muted = styles.getPropertyValue('--muted').trim();
  const green = styles.getPropertyValue('--green').trim();
  const text = styles.getPropertyValue('--text').trim();
  const ctx = canvas.getContext('2d');
  const gradient = ctx.createLinearGradient(0, 0, 0, 260);
  gradient.addColorStop(0, 'rgba(40, 209, 124, .30)');
  gradient.addColorStop(1, 'rgba(40, 209, 124, 0)');
  revenueChart = new Chart(ctx, {
    type: 'line',
    data: {
      labels: points.map(point => point.date),
      datasets: [{
        label: 'Revenue',
        data: points.map(point => point.amount),
        borderColor: green,
        backgroundColor: gradient,
        borderWidth: 2,
        pointRadius: 3,
        pointHoverRadius: 6,
        pointBackgroundColor: green,
        tension: .35,
        fill: true,
      }],
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      interaction: { intersect: false, mode: 'index' },
      plugins: {
        legend: { display: false },
        tooltip: {
          displayColors: false,
          callbacks: { label: context => ` ${money(context.parsed.y)}` },
        },
      },
      scales: {
        x: { grid: { display: false }, ticks: { color: muted, font: { size: 10 } } },
        y: { beginAtZero: true, border: { display: false }, grid: { color: line }, ticks: { color: muted, font: { size: 10 }, callback: value => money(value) } },
      },
      animation: { duration: 450 },
      color: text,
    },
  });
}

async function renderSales() {
  [state.transactions, state.overview] = await Promise.all([api('/transactions'), api('/overview')]);
  const o = state.overview;
  $('#app').innerHTML = `${pageHead('Revenue ledger', 'Sales & inventory', 'Every peso, session, and station in one searchable ledger.', `<button class="secondary-btn" id="export-sales">⇩ Export CSV</button>`)}
  <section class="stats-grid">
    <article class="stat-card"><small>Total sales</small><strong>${money(o.totalSales)}</strong><footer>All recorded time</footer></article>
    <article class="stat-card"><small>Transactions</small><strong>${o.transactionCount}</strong><footer>Coins and vouchers</footer></article>
    <article class="stat-card"><small>Average sale</small><strong>${money(o.transactionCount ? o.totalSales/o.transactionCount : 0)}</strong><footer>Per transaction</footer></article>
    <article class="stat-card"><small>This month</small><strong>${money(o.monthSales)}</strong><footer>Rolling 30 days</footer></article>
  </section>
  <section class="card table-card"><div class="table-tools"><div><strong>Transaction history</strong><small style="display:block">Newest payments first</small></div><input class="search" id="sales-search" placeholder="Search IP, MAC, or type…"></div><div class="table-wrap" id="sales-table"></div></section>`;
  renderSalesRows(state.transactions);
  $('#sales-search').oninput = e => { const q=e.target.value.toLowerCase(); renderSalesRows(state.transactions.filter(tx => JSON.stringify(tx).toLowerCase().includes(q))); };
  $('#export-sales').onclick = exportSales;
}
function renderSalesRows(rows) {
  $('#sales-table').innerHTML = rows.length ? `<table class="table table-hover align-middle mb-0"><thead><tr><th>Date</th><th>Type</th><th>Station</th><th>IP address</th><th>MAC</th><th>Time</th><th>Amount</th></tr></thead><tbody>${rows.map(tx => `<tr><td>${dateTime(tx.createdAt)}</td><td>${badge(tx.kind, tx.kind === 'Voucher' ? 'orange':'blue')}</td><td>${tx.station}</td><td class="mono">${tx.clientIp}</td><td class="mono">${tx.mac}</td><td>${duration(tx.minutes)}</td><td class="money">${money(tx.amount)}</td></tr>`).join('')}</tbody></table>` : empty('No matching payments','Try a different search.');
}
function exportSales() {
  const rows = [['Date','Type','Station','IP','MAC','Minutes','Amount'], ...state.transactions.map(t => [t.createdAt,t.kind,t.station,t.clientIp,t.mac,t.minutes,t.amount])];
  const csv = rows.map(r => r.map(v => `"${String(v).replaceAll('"','""')}"`).join(',')).join('\n');
  const a=document.createElement('a'); a.href=URL.createObjectURL(new Blob([csv],{type:'text/csv'})); a.download=`chasselfi-sales-${new Date().toISOString().slice(0,10)}.csv`; a.click(); URL.revokeObjectURL(a.href); toast('Sales export downloaded');
}

async function renderSessions() {
  state.sessions = await api('/sessions');
  const active = state.sessions.filter(s => s.status !== 'ended');
  $('#app').innerHTML = `${pageHead('Client control', 'Live sessions', 'Monitor time, usage, and connection status for every customer.', `<button class="secondary-btn" id="refresh-sessions">↻ Refresh</button>`)}
    <section class="stats-grid">
      <article class="stat-card"><small>Connected</small><strong>${active.filter(s=>s.status==='online').length}</strong><footer>Actively browsing</footer></article>
      <article class="stat-card"><small>Paused</small><strong>${active.filter(s=>s.status==='paused').length}</strong><footer>Time is preserved</footer></article>
      <article class="stat-card"><small>Download now</small><strong>${active.reduce((a,s)=>a+s.downloadMbps,0).toFixed(1)} Mbps</strong><footer>Across all clients</footer></article>
      <article class="stat-card"><small>Upload now</small><strong>${active.reduce((a,s)=>a+s.uploadMbps,0).toFixed(1)} Mbps</strong><footer>Across all clients</footer></article>
    </section>
    <section class="session-grid">${active.map(sessionCard).join('') || empty('No active clients','New sessions appear here automatically.')}</section>`;
  $('#refresh-sessions').onclick = () => renderPage('sessions');
}
function sessionCard(s) {
  return `<article class="session-card"><div class="session-top"><div class="device-icon">▣</div><div style="flex:1"><strong>${s.clientName}</strong><small style="display:block">${s.ip}</small></div>${badge(s.status, s.status==='paused'?'orange':'')}</div><div class="session-meta"><div><small>Time left</small><strong>${remaining(s.remainingSeconds)}</strong></div><div><small>Speed</small><strong>↓${s.downloadMbps} ↑${s.uploadMbps}</strong></div><div><small>MAC</small><strong>${s.mac.slice(-8)}</strong></div><div><small>Started</small><strong>${new Date(s.startedAt).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})}</strong></div></div><div class="session-actions">${s.status==='online'?`<button class="secondary-btn" data-session="${s.id}" data-action="pause">Pause</button>`:`<button class="secondary-btn" data-session="${s.id}" data-action="resume">Resume</button>`}<button class="danger-outline" data-session="${s.id}" data-action="stop">End</button></div></article>`;
}

async function renderRates() {
  state.rates = await api('/rates');
  $('#app').innerHTML = `${pageHead('Pricing', 'Timer rates', 'Create simple, transparent packages for your customers.', `<button class="primary-btn" id="add-rate">＋ Add rate</button>`)}
  <section class="rate-grid">${state.rates.map((r,i) => `<article class="rate-card ${i===1?'featured':''}"><div style="display:flex;justify-content:space-between"><span class="badge ${r.active?'':'red'}">${r.active?'Active':'Disabled'}</span><small>${r.label}</small></div><div class="price">${money(r.price)}</div><div class="duration">${duration(r.minutes)} access</div><hr><ul><li>${r.downloadMbps} Mbps download</li><li>${r.uploadMbps} Mbps upload</li><li>One device per session</li></ul><footer><button class="secondary-btn" data-edit-rate="${r.id}">Edit</button><button class="danger-outline" data-delete-rate="${r.id}">Delete</button></footer></article>`).join('')}</section>`;
  $('#add-rate').onclick = () => rateModal();
}
function rateModal(rate = {}) {
  openModal(rate.id ? 'Edit timer rate' : 'Add timer rate', `<form class="modal-form needs-validation" id="rate-form"><div class="form-grid"><div class="field"><label class="form-label">Price (₱)</label><input class="form-control" name="price" type="number" min="1" required value="${rate.price||''}"></div><div class="field"><label class="form-label">Duration (minutes)</label><input class="form-control" name="minutes" type="number" min="1" required value="${rate.minutes||''}"></div><div class="field"><label class="form-label">Download (Mbps)</label><input class="form-control" name="downloadMbps" type="number" min="1" required value="${rate.downloadMbps||15}"></div><div class="field"><label class="form-label">Upload (Mbps)</label><input class="form-control" name="uploadMbps" type="number" min="1" required value="${rate.uploadMbps||10}"></div><div class="field full"><label class="form-label">Customer-facing label</label><input class="form-control" name="label" maxlength="40" required value="${rate.label||''}" placeholder="e.g. Best value"></div></div><label class="toggle-row"><span><strong>Offer this rate</strong><small>Visible on the customer portal</small></span><span class="form-check form-switch"><input class="form-check-input" name="active" type="checkbox" role="switch" ${rate.active!==false?'checked':''}></span></label><div class="modal-actions"><button type="button" class="btn secondary-btn" data-close-modal>Cancel</button><button class="btn primary-btn">${rate.id?'Save changes':'Create rate'}</button></div></form>`);
  $('#rate-form').onsubmit = async e => { e.preventDefault(); const f=new FormData(e.target); const body={price:+f.get('price'),minutes:+f.get('minutes'),downloadMbps:+f.get('downloadMbps'),uploadMbps:+f.get('uploadMbps'),label:f.get('label'),active:f.get('active')==='on'}; await api(rate.id?`/rates/${rate.id}`:'/rates',{method:rate.id?'PUT':'POST',body:JSON.stringify(body)}); closeModal(); toast(rate.id?'Rate updated':'Rate created'); renderPage('rates'); };
}

async function renderVouchers() {
  state.vouchers = await api('/vouchers');
  $('#app').innerHTML = `${pageHead('Prepaid access', 'Voucher generator', 'Create printable access codes for resellers and walk-in customers.', `<button class="btn secondary-btn" id="print-vouchers">▣ Print ready codes</button><button class="primary-btn" id="generate-vouchers">＋ Generate batch</button>`)}
  <section class="stats-grid"><article class="stat-card"><small>Total codes</small><strong>${state.vouchers.length}</strong><footer>All batches</footer></article><article class="stat-card"><small>Ready</small><strong>${state.vouchers.filter(v=>v.status==='ready').length}</strong><footer>Unused inventory</footer></article><article class="stat-card"><small>Redeemed</small><strong>${state.vouchers.filter(v=>v.status==='used').length}</strong><footer>Completed sales</footer></article><article class="stat-card"><small>Inventory value</small><strong>${money(state.vouchers.filter(v=>v.status==='ready').reduce((a,v)=>a+v.price,0))}</strong><footer>Ready codes</footer></article></section>
  <section class="card table-card"><div class="table-tools"><div><strong>Voucher inventory</strong><small style="display:block">Codes are stored locally on this vendo</small></div><input class="search" id="voucher-search" placeholder="Search code or batch…"></div><div class="table-wrap" id="voucher-table"></div></section>`;
  renderVoucherRows(state.vouchers); $('#voucher-search').oninput=e=>{const q=e.target.value.toLowerCase();renderVoucherRows(state.vouchers.filter(v=>v.code.toLowerCase().includes(q)||v.batch.toLowerCase().includes(q)));}; $('#generate-vouchers').onclick=voucherModal; $('#print-vouchers').onclick=printVouchers;
}
function renderVoucherRows(rows) {
  $('#voucher-table').innerHTML = rows.length ? `<table class="table table-hover align-middle mb-0"><thead><tr><th>Code</th><th>Status</th><th>Access time</th><th>Price</th><th>Batch</th><th>Created</th><th></th></tr></thead><tbody>${rows.map(v=>`<tr><td class="mono"><strong>${v.code}</strong></td><td>${badge(v.status,v.status==='used'?'red':'')}</td><td>${duration(v.minutes)}</td><td class="money">${money(v.price)}</td><td class="mono">${v.batch}</td><td>${dateTime(v.createdAt)}</td><td><div class="row-actions"><button class="btn" title="Copy" data-copy="${v.code}">⧉</button><button class="btn" title="Delete" data-delete-voucher="${v.id}">×</button></div></td></tr>`).join('')}</tbody></table>` : empty('No vouchers yet','Generate your first batch to start selling prepaid access.');
}
function voucherModal() {
  openModal('Generate voucher batch', `<form class="modal-form needs-validation" id="voucher-form"><div class="form-grid"><div class="field"><label class="form-label">Quantity</label><input class="form-control" name="quantity" type="number" min="1" max="100" value="10" required></div><div class="field"><label class="form-label">Price each (₱)</label><input class="form-control" name="price" type="number" min="0" value="10" required></div><div class="field"><label class="form-label">Access time (minutes)</label><input class="form-control" name="minutes" type="number" min="1" value="120" required></div><div class="field"><label class="form-label">Expires after (days)</label><input class="form-control" name="expiresInDays" type="number" min="1" value="30"></div></div><div class="modal-actions"><button type="button" class="btn secondary-btn" data-close-modal>Cancel</button><button class="btn primary-btn">Generate codes</button></div></form>`);
  $('#voucher-form').onsubmit=async e=>{e.preventDefault();const f=new FormData(e.target),body=Object.fromEntries([...f].map(([k,v])=>[k,+v]));const codes=await api('/vouchers/generate',{method:'POST',body:JSON.stringify(body)});closeModal();toast(`${codes.length} vouchers generated`);renderPage('vouchers');};
}
function printVouchers(){const ready=state.vouchers.filter(v=>v.status==='ready');if(!ready.length)return toast('No ready vouchers to print','error');const printWindow=window.open('','_blank','width=800,height=900');printWindow.document.write(`<title>ChasselFi vouchers</title><style>body{font:16px Arial;padding:24px}h1{margin-bottom:4px}.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:12px}.voucher{border:1px dashed #555;padding:16px;text-align:center;border-radius:8px}.code{font:700 24px monospace;letter-spacing:3px}.meta{color:#555;margin-top:8px}@media print{button{display:none}}</style><h1>ChasselFi vouchers</h1><p>Print date: ${new Date().toLocaleString()}</p><div class="grid">${ready.map(v=>`<div class="voucher"><div class="code">${v.code}</div><div class="meta">${duration(v.minutes)} · ${money(v.price)}</div></div>`).join('')}</div>`);printWindow.document.close();printWindow.focus();setTimeout(()=>printWindow.print(),250);}

async function renderNetwork() {
  const [system, router, interfaces, discovery] = await Promise.all([api('/system'), api('/router/status'), api('/network/interfaces'), api('/network/discovery')]);
  state.system = system;
  state.networkDiscovery = discovery;
  const liveInterfaces = discovery.interfaces?.length ? discovery.interfaces : interfaces;
  const wan = liveInterfaces.find(item=>item.name===discovery.recommendedWan);
  const clientLan = liveInterfaces.find(item=>item.addresses?.includes('10.0.0.1/20')) || liveInterfaces.find(item=>item.name.endsWith('.799'));
  const shapingInterface = clientLan?.name || discovery.recommendedLan || '';
  $('#app').innerHTML = `${pageHead('Router control', 'Network status', 'A clear view of interfaces, throughput limits, and the service state.', `<button class="secondary-btn" id="network-refresh">↻ Refresh</button>`)}
  <section class="network-hero">
    <article class="stat-card network-main"><small>Customer LAN</small><strong>${clientLan?.addresses?.join(', ')||'10.0.0.1/20'}</strong><footer>${badge(clientLan?.state==='up'?`Active · ${clientLan.name}`:'Not detected',clientLan?.state==='up'?'':'orange')}</footer></article>
    <article class="stat-card"><small>WAN · ${wan?.name||'not detected'}</small><strong class="${wan?.state==='up'?'trend-up':'text-danger'}">${wan?.state==='up'?'Online':'Offline'}</strong><footer>${wan?.addresses?.join(', ')||'No address'}</footer></article>
    <article class="stat-card"><small>Clients</small><strong>${state.system.onlineUsers}</strong><footer>Authenticated now</footer></article>
    <article class="stat-card"><small>Mode</small><strong style="font-size:20px">${state.system.hardwareMode}</strong><footer>Hardware adapter</footer></article>
  </section>
  <section class="card"><div class="card-head"><div><h2>Detected interfaces</h2><p>Live Linux interface telemetry; map WAN and LAN in the router templates</p></div></div><div class="interface-list">
    ${(liveInterfaces.length ? liveInterfaces : [{name:'No Linux interfaces detected',state:'simulated',mac:null,addresses:[],kind:'unknown',rxBytes:0,txBytes:0}]).map(item=>interfaceRow(item.name,item.kind||'Linux interface',item.addresses?.join(', ')||'No IP address',`${item.mac||'No MAC'} · RX ${formatBytes(item.rxBytes)} · TX ${formatBytes(item.txBytes)}`,item.state==='up'?'Online':item.state)).join('')}
  </div></section>
  <section class="card"><div class="card-head"><div><h2>Automatic WAN / LAN discovery</h2><p>Server mode only: ChasselFi recommends a mapping but never changes the host network automatically.</p></div>${badge(discovery.containerized?'Container view':`${discovery.confidence} confidence`, discovery.containerized||discovery.confidence!=='high'?'orange':'')}</div>
    <p class="network-reason">${discovery.reason}</p>
    <div class="network-discovery-grid"><div class="interface"><div><small>Recommended WAN</small><strong class="mono">${discovery.recommendedWan||'Not found'}</strong></div><div><small>Detection rule</small><strong>Default route</strong></div>${badge(discovery.recommendedWan?'Review':'Missing', discovery.recommendedWan?'':'red')}</div>
    <div class="interface"><div><small>Recommended LAN</small><strong class="mono">${discovery.recommendedLan||'Not found'}</strong></div><div><small>Detection rule</small><strong>${discovery.recommendedLan && discovery.interfaces.find(item=>item.name===discovery.recommendedLan)?.usb?'USB Ethernet':'Remaining Ethernet'}</strong></div>${badge(discovery.recommendedLan?'Review':'Missing', discovery.recommendedLan?'':'red')}</div></div>
    <form id="network-plan-form" class="form-grid" style="margin-top:16px"><div class="field"><label class="form-label">WAN interface</label><input class="form-control" name="wanInterface" maxlength="15" value="${discovery.recommendedWan||''}" placeholder="e.g. eth0" required></div><div class="field"><label class="form-label">LAN interface</label><input class="form-control" name="lanInterface" maxlength="15" value="${discovery.recommendedLan||''}" placeholder="e.g. enx..." required></div><div class="field"><label class="form-label">Client gateway</label><input class="form-control" name="lanAddress" value="10.0.0.1" required></div><div class="field"><label class="form-label">Prefix length</label><input class="form-control" name="lanPrefix" type="number" min="8" max="30" value="20" required></div><div class="field full"><button class="btn primary-btn">Generate review plan</button></div></form><pre class="network-plan-output" id="network-plan-output" hidden></pre>
  </section>
  <div class="two-col"><section class="card"><div class="card-head"><div><h2>Per-session bandwidth</h2><p>Enforced by openNDS when a voucher connects</p></div></div>${metric('Download',`${state.settings?.downloadLimitMbps||15} Mbps`,65)}${metric('Upload',`${state.settings?.uploadLimitMbps||10} Mbps`,45)}</section><section class="card"><div class="card-head"><div><h2>Global shaping preview</h2><p>${router.message}</p></div>${badge(router.mode, router.liveApplyEnabled ? '' : 'orange')}</div><div class="interface-list"><div class="interface"><div><strong>tc</strong><small>${router.tcAvailable ? 'Available' : 'Not detected'}</small></div><div><strong>nftables</strong><small>${router.nftAvailable ? 'Available' : 'Not detected'}</small></div><div><strong>dnsmasq</strong><small>${router.dnsmasqAvailable ? 'Available' : 'Not detected'}</small></div><div>${badge(router.liveApplyEnabled ? 'Live-capable' : 'Preview only', router.liveApplyEnabled ? '' : 'orange')}</div></div></div><form id="shape-form" class="form-grid" style="margin-top:16px"><div class="field"><label class="form-label">Interface</label><input class="form-control" name="interface" value="${shapingInterface}" maxlength="15" required></div><div class="field"><label class="form-label">Download Mbps</label><input class="form-control" name="downloadMbps" type="number" min="1" value="${state.settings?.downloadLimitMbps||15}" required></div><div class="field"><label class="form-label">Upload Mbps</label><input class="form-control" name="uploadMbps" type="number" min="1" value="${state.settings?.uploadLimitMbps||10}" required></div><div class="field" style="align-self:end"><button class="btn primary-btn w-100">Validate shaping plan</button></div></form></section></div>`;
  $('#network-refresh').onclick=()=>renderPage('network');
  $('#network-plan-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target);const result=await api('/network/plan',{method:'POST',body:JSON.stringify({wanInterface:form.get('wanInterface'),lanInterface:form.get('lanInterface'),lanAddress:form.get('lanAddress'),lanPrefix:+form.get('lanPrefix')})});const output=$('#network-plan-output');output.hidden=false;output.textContent=`${result.message}\n\n${result.commands.map(command=>`$ ${command.join(' ')}`).join('\n')}`;toast('Network mapping validated; no changes applied');};
  $('#shape-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target);const result=await api('/router/apply',{method:'POST',body:JSON.stringify({interface:form.get('interface'),downloadMbps:+form.get('downloadMbps'),uploadMbps:+form.get('uploadMbps'),dryRun:true})});toast(result.message);};
}
function interfaceRow(name,protocol,ip,role,status){return `<div class="interface"><div><strong>${name}</strong><small>${role}</small></div><div><small>Protocol</small><strong>${protocol}</strong></div><div><small>Address / MAC</small><strong class="mono">${ip}</strong></div>${badge(status)}</div>`;}
function formatBytes(bytes){if(!bytes)return '0 B';const units=['B','KB','MB','GB','TB'];const index=Math.min(Math.floor(Math.log(bytes)/Math.log(1024)),units.length-1);return `${(bytes/1024**index).toFixed(index?1:0)} ${units[index]}`;}

async function renderTools() {
  state.sites = await api('/blocked-sites');
  $('#app').innerHTML = `${pageHead('Access policy', 'Site blocking', 'Enforce a DNS deny list across the VLAN 799 customer network.')}
  <div class="two-col"><section class="card table-card"><div class="table-tools"><div><strong>Blocked destinations</strong><small style="display:block">Enforced by gateway DNS; encrypted DNS apps may bypass DNS filtering</small></div><input class="search" id="site-search" placeholder="Search rules…"></div><div class="table-wrap" id="site-table"></div></section>
  <section class="card"><div class="card-head"><div><h2>Quick block</h2><p>Add a DNS hostname</p></div></div><form class="modal-form" id="site-form"><div class="field"><label>Hostname</label><input name="host" required placeholder="example.com"></div><div class="field"><label>Reason (optional)</label><textarea name="note" rows="3" placeholder="Why this rule exists"></textarea></div><button class="primary-btn">＋ Add block rule</button></form></section></div>`;
  renderSiteRows(state.sites); $('#site-search').oninput=e=>{let q=e.target.value.toLowerCase();renderSiteRows(state.sites.filter(x=>JSON.stringify(x).toLowerCase().includes(q)));};
  $('#site-form').onsubmit=async e=>{e.preventDefault();const body=Object.fromEntries(new FormData(e.target));await api('/blocked-sites',{method:'POST',body:JSON.stringify(body)});toast('Block rule queued for gateway enforcement');renderPage('tools');};
}
function renderSiteRows(rows){$('#site-table').innerHTML=rows.length?`<table class="table table-hover align-middle mb-0"><thead><tr><th>Destination</th><th>Note</th><th>Added</th><th></th></tr></thead><tbody>${rows.map(x=>`<tr><td class="mono"><strong>${x.host}</strong></td><td>${x.note||'—'}</td><td>${dateTime(x.createdAt)}</td><td><button class="btn danger-outline" data-delete-site="${x.id}">Remove</button></td></tr>`).join('')}</tbody></table>`:empty('Nothing blocked','The deny list is currently empty.');}

async function renderSettings() {
  state.settings = await api('/settings'); const s=state.settings;
  $('#app').innerHTML = `${pageHead('Configuration', 'Settings', 'Tune the portal, limits, and daily behavior of your vendo.', '<button class="btn secondary-btn" id="download-backup">⇩ Download backup</button><button class="btn secondary-btn" id="restore-backup">↥ Restore backup</button><input id="backup-file" type="file" accept="application/json,.json" hidden>')}
  <form id="settings-form"><div class="settings-grid"><section class="setting-card"><h2>Shop identity</h2><p>Customer-facing details</p><div class="form-grid"><div class="field full"><label>Shop name</label><input name="shopName" value="${s.shopName}" required></div><div class="field full"><label>Portal message</label><textarea name="portalMessage" rows="3">${s.portalMessage}</textarea></div><div class="field"><label>Timezone</label><select name="timezone"><option>Asia/Manila</option><option>Asia/Singapore</option><option>UTC</option></select></div><div class="field"><label>Currency</label><select name="currency"><option value="PHP">Philippine peso</option></select></div></div></section>
  <section class="setting-card"><h2>Payment mode</h2><p>Choose the real customer payment methods</p><div class="field"><label>Accepted payment</label><select name="paymentMode"><option value="voucher">Voucher only</option><option value="coin">Coin only</option><option value="both">Coin and voucher</option></select></div><div class="field" style="margin-top:14px"><label>Value per hardware pulse (peso)</label><input name="coinPulseValue" type="number" min="1" max="100" value="${s.coinPulseValue||1}"><small>Use 1 for a standard ₱1 pulse acceptor.</small></div>${toggle('autoPause','Brownout auto-pause','Preserve remaining time after interruption',s.autoPause)}</section>
  <section class="setting-card"><h2>Speed limits</h2><p>Default per-user bandwidth</p><div class="form-grid"><div class="field"><label>Download (Mbps)</label><input name="downloadLimitMbps" type="number" min="1" value="${s.downloadLimitMbps}"></div><div class="field"><label>Upload (Mbps)</label><input name="uploadLimitMbps" type="number" min="1" value="${s.uploadLimitMbps}"></div></div></section>
  <section class="setting-card"><h2>Maintenance</h2><p>Scheduled service behavior</p>${toggle('maintenanceSchedule','Scheduled maintenance','Enable the daily maintenance window',s.maintenanceSchedule)}<div class="field" style="margin-top:14px"><label>Window</label><input value="03:00 Asia/Manila" disabled></div></section></div><div class="save-bar"><button class="primary-btn">Save all changes</button></div></form>`;
  $(`[name="timezone"]`).value=s.timezone;
  $(`[name="paymentMode"]`).value=s.paymentMode||'voucher';
  $('#settings-form').onsubmit=saveSettings;
  $('#download-backup').onclick=downloadBackup;
  $('#restore-backup').onclick=()=>$('#backup-file').click();
  $('#backup-file').onchange=restoreBackup;
}
function toggle(name,title,desc,on){return `<label class="toggle-row"><span><strong>${title}</strong><small>${desc}</small></span><span class="switch"><input type="checkbox" name="${name}" ${on?'checked':''}><i></i></span></label>`;}
async function saveSettings(e){e.preventDefault();const f=new FormData(e.target),mode=f.get('paymentMode'),body={shopName:f.get('shopName'),timezone:f.get('timezone'),currency:f.get('currency'),portalMessage:f.get('portalMessage'),paymentMode:mode,coinPulseValue:+f.get('coinPulseValue'),buyTime:mode==='coin'||mode==='both',vouchers:mode==='voucher'||mode==='both',autoPause:f.get('autoPause')==='on',downloadLimitMbps:+f.get('downloadLimitMbps'),uploadLimitMbps:+f.get('uploadLimitMbps'),maintenanceSchedule:f.get('maintenanceSchedule')==='on'};await api('/settings',{method:'PUT',body:JSON.stringify(body)});state.settings=body;toast('Payment mode and settings saved');}
async function downloadBackup(){const backup=await api('/backup');const link=document.createElement('a');link.href=URL.createObjectURL(new Blob([JSON.stringify(backup,null,2)],{type:'application/json'}));link.download=`chasselfi-backup-${new Date().toISOString().slice(0,10)}.json`;link.click();URL.revokeObjectURL(link.href);toast('Backup downloaded');}
async function restoreBackup(event){const file=event.target.files?.[0];if(!file)return;try{const backup=JSON.parse(await file.text());await api('/backup/restore',{method:'POST',body:JSON.stringify(backup)});toast('Backup restored. Reloading…');setTimeout(()=>location.reload(),600);}catch(error){toast(error.message,'error');}event.target.value='';}

async function renderPage(forced) {
  const page = forced || location.hash.slice(1) || 'dashboard';
  if (!pageNames[page]) { location.hash='dashboard'; return; }
  $('#breadcrumb').textContent = pageNames[page];
  $$('[data-page]').forEach(a => a.classList.toggle('active', a.dataset.page === page));
  $('#sidebar').classList.remove('open');
  $('#app').innerHTML='<div class="loading-state"><div class="spinner"></div><p>Loading…</p></div>';
  try {
    if (page === 'dashboard') await renderDashboard();
    else if (page === 'sales') await renderSales();
    else if (page === 'sessions') await renderSessions();
    else if (page === 'rates') await renderRates();
    else if (page === 'vouchers') await renderVouchers();
    else if (page === 'network') { if(!state.settings) state.settings=await api('/settings'); await renderNetwork(); }
    else if (page === 'tools') await renderTools();
    else if (page === 'settings') await renderSettings();
  } catch (error) {
    if (error.authRequired) return;
    console.error(error); $('#app').innerHTML=`${pageHead('Connection problem','Dashboard unavailable','The Rust service did not answer this request.')}<section class="card">${empty('Could not load data',error.message)}<button class="primary-btn" onclick="location.reload()">Try again</button></section>`;
  }
}

document.addEventListener('click', async event => {
  try {
  const close = event.target.closest('[data-close-modal]'); if (close) return closeModal();
  const session = event.target.closest('[data-session]'); if(session){await api(`/sessions/${session.dataset.session}/${session.dataset.action}`,{method:'POST'});toast(`Session ${session.dataset.action}d`);return renderPage('sessions');}
  const editRate=event.target.closest('[data-edit-rate]'); if(editRate)return rateModal(state.rates.find(r=>r.id===editRate.dataset.editRate));
  const deleteRate=event.target.closest('[data-delete-rate]'); if(deleteRate&&confirm('Delete this timer rate?')){await api(`/rates/${deleteRate.dataset.deleteRate}`,{method:'DELETE'});toast('Rate deleted');return renderPage('rates');}
  const delVoucher=event.target.closest('[data-delete-voucher]');if(delVoucher&&confirm('Delete this voucher?')){await api(`/vouchers/${delVoucher.dataset.deleteVoucher}`,{method:'DELETE'});toast('Voucher deleted');return renderPage('vouchers');}
  const copy=event.target.closest('[data-copy]');if(copy){await navigator.clipboard.writeText(copy.dataset.copy);return toast('Voucher code copied');}
  const delSite=event.target.closest('[data-delete-site]');if(delSite){await api(`/blocked-sites/${delSite.dataset.deleteSite}`,{method:'DELETE'});toast('Block rule queued for removal');return renderPage('tools');}
  const system=event.target.closest('[data-system-action]');if(system&&confirm(`Send a ${system.dataset.systemAction} request?`)){const result=await api(`/system/${system.dataset.systemAction}`,{method:'POST'});toast(result.message||'System action accepted');}
  } catch (error) {
    if (!error.authRequired) toast(error.message, 'error');
  }
});

document.addEventListener('keydown',e=>{if(e.key==='Escape')closeModal();});
$('#menu-btn').onclick=()=>$('#sidebar').classList.toggle('open');
$('#logout-btn').onclick=async()=>{await api('/logout',{method:'POST'}).catch(()=>{});localStorage.removeItem('chasselfiCsrf');showLogin();};
$('#theme-btn').onclick=()=>{const next=document.documentElement.dataset.theme==='light'?'dark':'light';document.documentElement.dataset.theme=next;document.body.dataset.bsTheme=next;localStorage.setItem('theme',next);renderPage();};
document.documentElement.dataset.theme=localStorage.getItem('theme')||'dark';
document.body.dataset.bsTheme=document.documentElement.dataset.theme;
window.addEventListener('hashchange',()=>renderPage());
window.addEventListener('resize',()=>{if(location.hash==='#dashboard'&&state.overview)requestAnimationFrame(()=>drawRevenueChart($('#revenue-chart'),state.overview.dailySales));});
renderPage();
