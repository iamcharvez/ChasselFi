const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const state = { overview: null, system: null, business: null, rates: [], sessions: [], vouchers: [], transactions: [], sites: [], settings: null, coinNodes: [] };
let revenueChart = null;
let bandwidthChart = null;
let bandwidthTimer = null;
let loginVisible = false;
const pageNames = { dashboard: 'Dashboard', sales: 'Sales & inventory', sessions: 'Live sessions', rates: 'Timer rates', vouchers: 'Voucher studio', 'free-time': 'Free time', 'coin-nodes': 'Coin nodes', network: 'Network status', tools: 'Site blocking', 'portal-design': 'Portal designer', settings: 'Settings' };

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
  const raw = response.status === 204 ? '' : await response.text();
  let data = null;
  if (raw) {
    try { data = JSON.parse(raw); }
    catch { throw new Error(`Invalid server response from ${path} (${response.status})`); }
  }
  if (response.status === 401 && path !== '/login') {
    showLogin();
    const error = new Error('Login required');
    error.authRequired = true;
    throw error;
  }
  if (!response.ok) throw new Error(data?.error || `Request failed (${response.status})`);
  if (response.status !== 204 && data === null) throw new Error(`Empty server response from ${path} (${response.status})`);
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
const esc = value => String(value ?? '').replace(/[&<>'"]/g, character => ({'&':'&amp;','<':'&lt;','>':'&gt;',"'":'&#39;','"':'&quot;'}[character]));
const dateTime = value => new Intl.DateTimeFormat('en-PH', { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(value));
const duration = minutes => minutes >= 1440 ? `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h` : minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60 ? `${minutes % 60}m` : ''}` : `${minutes}m`;
const remaining = seconds => `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m`;
const imageDataUrl = file => new Promise((resolve, reject) => {
  if (!file?.type?.startsWith('image/')) return reject(new Error('Choose a PNG, JPEG, WebP, GIF, or SVG image.'));
  if (file.size > 1_800_000) return reject(new Error('Image must be smaller than 1.8 MB.'));
  const reader = new FileReader();
  reader.onload = () => resolve(reader.result);
  reader.onerror = () => reject(new Error('Could not read that image.'));
  reader.readAsDataURL(file);
});

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
        <section class="card"><div class="card-head"><div><h2>Live bandwidth</h2><p>Measured from the customer VLAN interface every three seconds</p></div><span class="badge" id="bandwidth-interface">Detecting…</span></div><div class="bandwidth-now"><span><i class="down"></i><small>DOWNLOAD</small><strong id="bandwidth-rx">0 Kbps</strong></span><span><i class="up"></i><small>UPLOAD</small><strong id="bandwidth-tx">0 Kbps</strong></span></div><div class="chart-wrap bandwidth-chart"><canvas id="bandwidth-chart"></canvas></div></section>
      </div>
      <div>
        <section class="card">
          <div class="card-head"><div><h2>Vendo health</h2><p>Live system resources</p></div>${badge('Online')}</div>
          <div class="system-list">
            ${metric('CPU', `${Number(s.cpuPercent).toFixed(1)}%`, Math.min(s.cpuPercent,100))}
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
  requestAnimationFrame(startBandwidthChart);
  state.rates = await api('/rates');
  $('#refresh-dashboard').onclick = () => renderPage('dashboard');
}

function formatRate(bytesPerSecond){const bits=bytesPerSecond*8;if(bits>=1e6)return `${(bits/1e6).toFixed(2)} Mbps`;return `${Math.round(bits/1e3)} Kbps`;}
async function startBandwidthChart(){
  clearInterval(bandwidthTimer);if(bandwidthChart)bandwidthChart.destroy();
  const canvas=$('#bandwidth-chart');if(!canvas||typeof Chart==='undefined')return;
  const colors=getComputedStyle(document.documentElement),green=colors.getPropertyValue('--green').trim(),blue=colors.getPropertyValue('--blue').trim(),line=colors.getPropertyValue('--line').trim(),muted=colors.getPropertyValue('--muted').trim();
  const data={labels:[],datasets:[{label:'Download Mbps',data:[],borderColor:green,backgroundColor:'rgba(40,209,124,.12)',fill:true,tension:.35,pointRadius:0},{label:'Upload Mbps',data:[],borderColor:blue,backgroundColor:'transparent',tension:.35,pointRadius:0}]};
  bandwidthChart=new Chart(canvas.getContext('2d'),{type:'line',data,options:{responsive:true,maintainAspectRatio:false,animation:false,plugins:{legend:{labels:{color:muted,usePointStyle:true,boxWidth:8}}},scales:{x:{display:false},y:{beginAtZero:true,grid:{color:line},ticks:{color:muted,callback:value=>`${value} Mb`}}}}});
  let previous=null;
  const poll=async()=>{try{const interfaces=await api('/network/interfaces'),item=interfaces.find(entry=>entry.name.endsWith('.799'))||interfaces.find(entry=>entry.name!=='lo'&&entry.state==='up');if(!item)return;$('#bandwidth-interface').textContent=item.name;if(previous){const elapsed=(Date.now()-previous.at)/1000,rx=Math.max(0,(item.rxBytes-previous.rx)/elapsed),tx=Math.max(0,(item.txBytes-previous.tx)/elapsed);$('#bandwidth-rx').textContent=formatRate(rx);$('#bandwidth-tx').textContent=formatRate(tx);data.labels.push(new Date().toLocaleTimeString());data.datasets[0].data.push(rx*8/1e6);data.datasets[1].data.push(tx*8/1e6);if(data.labels.length>40){data.labels.shift();data.datasets.forEach(set=>set.data.shift());}bandwidthChart.update('none');}previous={rx:item.rxBytes,tx:item.txBytes,at:Date.now()};}catch{}};
  await poll();bandwidthTimer=setInterval(poll,3000);
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
  $('#app').innerHTML = `${pageHead('Client command center', 'Connected users', 'Pause, extend, throttle, rename, resume, or revoke a customer from one screen.', `<button class="secondary-btn" id="refresh-sessions">↻ Refresh live data</button>`)}
    <section class="stats-grid">
      <article class="stat-card"><small>Connected</small><strong>${active.filter(s=>s.status==='online').length}</strong><footer>Actively browsing</footer></article>
      <article class="stat-card"><small>Paused</small><strong>${active.filter(s=>s.status==='paused').length}</strong><footer>Time is preserved</footer></article>
      <article class="stat-card"><small>Combined download cap</small><strong>${active.reduce((a,s)=>a+s.downloadMbps,0).toFixed(1)} Mbps</strong><footer>Configured client limits</footer></article>
      <article class="stat-card"><small>Combined upload cap</small><strong>${active.reduce((a,s)=>a+s.uploadMbps,0).toFixed(1)} Mbps</strong><footer>Configured client limits</footer></article>
    </section>
    <section class="client-toolbar"><div><strong>${active.length} active device${active.length===1?'':'s'}</strong><small>Changes are applied immediately through openNDS.</small></div><input class="search" id="session-search" placeholder="Search name, IP, or MAC…"></section>
    <section class="session-grid" id="session-grid">${active.map(sessionCard).join('') || empty('No active clients','New sessions appear here automatically.')}</section>`;
  $('#refresh-sessions').onclick = () => renderPage('sessions');
  $('#session-search').oninput = event => {
    const query = event.target.value.toLowerCase();
    const visible = active.filter(session => `${session.clientName} ${session.ip} ${session.mac}`.toLowerCase().includes(query));
    $('#session-grid').innerHTML = visible.map(sessionCard).join('') || empty('No matching client','Try a different name, address, or MAC.');
  };
}
function sessionCard(s) {
  const percent = Math.min(100, Math.max(4, s.remainingSeconds / 43200 * 100));
  return `<article class="session-card"><div class="session-top"><div class="device-icon">${s.status==='online'?'●':'Ⅱ'}</div><div style="flex:1"><strong>${esc(s.clientName)}</strong><small style="display:block">${esc(s.ip)}</small></div>${badge(s.status, s.status==='paused'?'orange':'')}</div><div class="session-time"><strong>${remaining(s.remainingSeconds)}</strong><span>remaining</span><div class="mini-meter"><i style="width:${percent}%"></i></div></div><div class="session-meta"><div><small>Download cap</small><strong>${s.downloadMbps} Mbps</strong></div><div><small>Upload cap</small><strong>${s.uploadMbps} Mbps</strong></div><div><small>MAC</small><strong>${esc(s.mac.slice(-8))}</strong></div><div><small>Started</small><strong>${new Date(s.startedAt).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'})}</strong></div></div><div class="session-actions"><button class="secondary-btn" data-edit-session="${s.id}">Edit / add time</button>${s.status==='online'?`<button class="secondary-btn" data-session="${s.id}" data-action="pause">Pause</button>`:`<button class="secondary-btn" data-session="${s.id}" data-action="resume">Resume</button>`}<button class="danger-outline" data-session="${s.id}" data-action="stop">Revoke</button></div></article>`;
}

function sessionModal(session) {
  openModal('Manage connected user', `<form class="modal-form" id="session-edit-form"><div class="modal-device-summary"><span class="device-icon">●</span><div><strong>${esc(session.clientName)}</strong><small>${esc(session.ip)} · ${esc(session.mac)}</small></div>${badge(session.status)}</div><div class="field"><label>Device label</label><input name="clientName" maxlength="64" value="${esc(session.clientName)}" required></div><div class="quick-time"><button type="button" data-add-minutes="15">+15 min</button><button type="button" data-add-minutes="60">+1 hour</button><button type="button" data-add-minutes="300">+5 hours</button></div><div class="field"><label>Remaining minutes</label><input name="remainingMinutes" type="number" min="0" max="43200" value="${Math.ceil(session.remainingSeconds/60)}" required><small>Set to zero to revoke access.</small></div><div class="form-grid"><div class="field"><label>Download Mbps</label><input name="downloadMbps" type="number" min="1" max="10000" value="${session.downloadMbps}" required></div><div class="field"><label>Upload Mbps</label><input name="uploadMbps" type="number" min="1" max="10000" value="${session.uploadMbps}" required></div></div><button class="primary-btn">Apply to gateway</button></form>`);
  $$('[data-add-minutes]', $('#modal-body')).forEach(button => button.onclick = () => { const input=$('[name="remainingMinutes"]',$('#modal-body')); input.value=+input.value + +button.dataset.addMinutes; });
  $('#session-edit-form').onsubmit = async event => { event.preventDefault(); const form=new FormData(event.target); await api(`/sessions/${session.id}`,{method:'PUT',body:JSON.stringify({clientName:form.get('clientName'),remainingMinutes:+form.get('remainingMinutes'),downloadMbps:+form.get('downloadMbps'),uploadMbps:+form.get('uploadMbps')})}); closeModal(); toast('Client policy updated on the gateway'); renderPage('sessions'); };
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
  [state.vouchers,state.settings] = await Promise.all([api('/vouchers'),api('/settings')]);
  $('#app').innerHTML = `${pageHead('Prepaid access', 'Voucher studio', 'Generate, brand, preview, and print access tickets ready for resale.', `<button class="btn secondary-btn" id="print-vouchers">▣ Print ready codes</button><button class="primary-btn" id="generate-vouchers">＋ Generate batch</button>`)}
  <section class="stats-grid"><article class="stat-card"><small>Total codes</small><strong>${state.vouchers.length}</strong><footer>All batches</footer></article><article class="stat-card"><small>Ready</small><strong>${state.vouchers.filter(v=>v.status==='ready').length}</strong><footer>Unused inventory</footer></article><article class="stat-card"><small>Redeemed</small><strong>${state.vouchers.filter(v=>v.status==='used').length}</strong><footer>Completed sales</footer></article><article class="stat-card"><small>Inventory value</small><strong>${money(state.vouchers.filter(v=>v.status==='ready').reduce((a,v)=>a+v.price,0))}</strong><footer>Ready codes</footer></article></section>
  <section class="voucher-studio card"><div><span class="eyebrow">PRINT DESIGN</span><h2>Voucher template</h2><p>Choose a professional ticket layout. Shop name, package, code, batch, and footer print automatically.</p><div class="template-picker"><button data-voucher-template="modern">Modern</button><button data-voucher-template="ticket">Ticket</button><button data-voucher-template="compact">Compact</button></div><div class="field"><label>Printed footer</label><input id="voucher-footer" maxlength="160" value="${esc(state.settings.voucherFooter||'Thank you for choosing ChasselFi WiFi')}"></div></div><div class="voucher-preview ${esc(state.settings.voucherTemplate||'modern')}" id="voucher-preview"><span>CHASSELFI ACCESS</span><strong>AB12CD34</strong><p>2 hours · ₱10</p><small>${esc(state.settings.voucherFooter||'Thank you for choosing ChasselFi WiFi')}</small></div></section>
  <section class="card table-card"><div class="table-tools"><div><strong>Voucher inventory</strong><small style="display:block">Codes are stored locally on this vendo</small></div><input class="search" id="voucher-search" placeholder="Search code or batch…"></div><div class="table-wrap" id="voucher-table"></div></section>`;
  renderVoucherRows(state.vouchers); $('#voucher-search').oninput=e=>{const q=e.target.value.toLowerCase();renderVoucherRows(state.vouchers.filter(v=>v.code.toLowerCase().includes(q)||v.batch.toLowerCase().includes(q)));}; $('#generate-vouchers').onclick=voucherModal; $('#print-vouchers').onclick=printVouchers;
  $$('[data-voucher-template]').forEach(button=>{button.classList.toggle('active',button.dataset.voucherTemplate===(state.settings.voucherTemplate||'modern'));button.onclick=async()=>{state.settings.voucherTemplate=button.dataset.voucherTemplate;state.settings.voucherFooter=$('#voucher-footer').value;await saveSettingsObject(state.settings);toast('Voucher design saved');renderPage('vouchers');};});
  $('#voucher-footer').onchange=async event=>{state.settings.voucherFooter=event.target.value;await saveSettingsObject(state.settings);toast('Voucher footer saved');renderPage('vouchers');};
}
function renderVoucherRows(rows) {
  $('#voucher-table').innerHTML = rows.length ? `<table class="table table-hover align-middle mb-0"><thead><tr><th>Code</th><th>Status</th><th>Access time</th><th>Price</th><th>Batch</th><th>Created</th><th></th></tr></thead><tbody>${rows.map(v=>`<tr><td class="mono"><strong>${v.code}</strong></td><td>${badge(v.status,v.status==='used'?'red':'')}</td><td>${duration(v.minutes)}</td><td class="money">${money(v.price)}</td><td class="mono">${v.batch}</td><td>${dateTime(v.createdAt)}</td><td><div class="row-actions"><button class="btn" title="Copy" data-copy="${v.code}">⧉</button><button class="btn" title="Delete" data-delete-voucher="${v.id}">×</button></div></td></tr>`).join('')}</tbody></table>` : empty('No vouchers yet','Generate your first batch to start selling prepaid access.');
}
function voucherModal() {
  openModal('Generate voucher batch', `<form class="modal-form needs-validation" id="voucher-form"><div class="form-grid"><div class="field"><label class="form-label">Quantity</label><input class="form-control" name="quantity" type="number" min="1" max="100" value="10" required></div><div class="field"><label class="form-label">Price each (₱)</label><input class="form-control" name="price" type="number" min="0" value="10" required></div><div class="field"><label class="form-label">Access time (minutes)</label><input class="form-control" name="minutes" type="number" min="1" value="120" required></div><div class="field"><label class="form-label">Expires after (days)</label><input class="form-control" name="expiresInDays" type="number" min="1" value="30"></div></div><div class="modal-actions"><button type="button" class="btn secondary-btn" data-close-modal>Cancel</button><button class="btn primary-btn">Generate codes</button></div></form>`);
  $('#voucher-form').onsubmit=async e=>{e.preventDefault();const f=new FormData(e.target),body=Object.fromEntries([...f].map(([k,v])=>[k,+v]));const codes=await api('/vouchers/generate',{method:'POST',body:JSON.stringify(body)});closeModal();toast(`${codes.length} vouchers generated`);renderPage('vouchers');};
}
function printVouchers(){const ready=state.vouchers.filter(v=>v.status==='ready');if(!ready.length)return toast('No ready vouchers to print','error');const template=state.settings?.voucherTemplate||'modern',footer=esc(state.settings?.voucherFooter||'Thank you for choosing ChasselFi WiFi'),shop=esc(state.settings?.shopName||'ChasselFi WiFi');const printWindow=window.open('','_blank','width=980,height=900');printWindow.document.write(`<title>${shop} vouchers</title><style>@page{margin:10mm}*{box-sizing:border-box}body{font-family:Inter,Arial,sans-serif;color:#101713;margin:0}.print-head{display:flex;justify-content:space-between;align-items:end;margin-bottom:18px}.print-head h1{margin:0}.grid{display:grid;grid-template-columns:repeat(${template==='compact'?3:2},1fr);gap:10px}.voucher{position:relative;overflow:hidden;border:1.5px ${template==='ticket'?'dashed':'solid'} #16251d;padding:${template==='compact'?'12px':'20px'};border-radius:${template==='modern'?'18px':'6px'};min-height:${template==='compact'?'125px':'175px'};display:flex;flex-direction:column;justify-content:space-between}.voucher:after{content:'';position:absolute;width:90px;height:90px;border-radius:50%;background:#25d67f22;right:-32px;top:-32px}.brand{font-size:10px;font-weight:800;letter-spacing:.16em;color:#187d50}.code{font:800 ${template==='compact'?'23px':'31px'} ui-monospace,monospace;letter-spacing:.15em;margin:12px 0}.meta{font-weight:750}.foot{font-size:9px;color:#68736d;margin-top:10px}.batch{font:10px ui-monospace;color:#68736d}</style><div class="print-head"><div><h1>${shop}</h1><p>Ready voucher inventory · ${new Date().toLocaleString()}</p></div><strong>${ready.length} tickets</strong></div><div class="grid">${ready.map(v=>`<div class="voucher"><div><div class="brand">${shop.toUpperCase()} · PREPAID WIFI</div><div class="code">${esc(v.code)}</div><div class="meta">${duration(v.minutes)} access · ${money(v.price)}</div></div><div><div class="foot">${footer}</div><div class="batch">Batch ${esc(v.batch)}</div></div></div>`).join('')}</div>`);printWindow.document.close();printWindow.focus();setTimeout(()=>printWindow.print(),350);}

async function saveSettingsObject(settings){await api('/settings',{method:'PUT',body:JSON.stringify(settings)});state.settings=settings;}

async function renderFreeTime(){
  [state.settings,state.transactions]=await Promise.all([api('/settings'),api('/transactions')]);
  const claims=state.transactions.filter(item=>item.kind==='Free time');
  $('#app').innerHTML=`${pageHead('Customer acquisition','Free time','Offer one controlled complimentary session per device, with terms and a configurable cooldown.')}
  <section class="stats-grid"><article class="stat-card"><small>Status</small><strong>${state.settings.freeTimeEnabled?'Enabled':'Disabled'}</strong><footer>Customer portal offer</footer></article><article class="stat-card"><small>Total claims</small><strong>${claims.length}</strong><footer>Recorded locally</footer></article><article class="stat-card"><small>Duration</small><strong>${state.settings.freeTimeMinutes} min</strong><footer>Per eligible device</footer></article><article class="stat-card"><small>Claim reset</small><strong>${state.settings.freeTimeResetHours}h</strong><footer>MAC/device cooldown</footer></article></section>
  <div class="two-col"><section class="card"><div class="card-head"><div><h2>Free-time policy</h2><p>Shown as the primary offer when enabled and enforced by openNDS</p></div>${badge(state.settings.freeTimeEnabled?'Live':'Off',state.settings.freeTimeEnabled?'':'orange')}</div><form id="free-time-form" class="modal-form">${toggle('freeTimeEnabled','Enable free time','Allow a free-time-only setup or combine it with coin and voucher access',state.settings.freeTimeEnabled)}<div class="form-grid"><div class="field"><label>Minutes per claim</label><input name="freeTimeMinutes" type="number" min="1" max="1440" value="${state.settings.freeTimeMinutes}"></div><div class="field"><label>Reset after hours</label><input name="freeTimeResetHours" type="number" min="1" max="8760" value="${state.settings.freeTimeResetHours}"></div></div>${toggle('requireTerms','Show terms before claim','The customer reads the terms in a modal, then accepts and claims with one button',state.settings.requireTerms)}<div class="field"><label>Terms title</label><input name="termsTitle" maxlength="100" value="${esc(state.settings.termsTitle)}"></div><div class="field"><label>Terms and conditions</label><textarea name="termsBody" maxlength="4000" rows="9">${esc(state.settings.termsBody)}</textarea><small>These terms apply only to Free Time. Vouchers never require this agreement.</small></div><button class="primary-btn">Save free-time policy</button></form></section><section class="card"><div class="card-head"><div><h2>Customer flow</h2><p>No pre-checked box and no browser-only timer</p></div></div><ol class="flow-list"><li><b>Customer taps Claim Free Time</b><span>The full terms open before any time is granted.</span></li><li><b>Customer reads and accepts</b><span>One explicit button accepts the terms and submits the claim.</span></li><li><b>Server checks eligibility</b><span>Recent claims for the same MAC/device are rejected.</span></li><li><b>Gateway authorizes</b><span>openNDS grants exactly the configured time and speed.</span></li><li><b>Time expires</b><span>ChasselFi deauthorizes the client automatically.</span></li></ol></section></div>
  <section class="card table-card"><div class="table-tools"><div><strong>Recent free-time claims</strong><small style="display:block">Auditable zero-peso transactions</small></div></div><div class="table-wrap">${claims.length?`<table class="table"><thead><tr><th>Claimed</th><th>IP</th><th>MAC</th><th>Duration</th></tr></thead><tbody>${claims.slice(0,50).map(item=>`<tr><td>${dateTime(item.createdAt)}</td><td class="mono">${esc(item.clientIp)}</td><td class="mono">${esc(item.mac)}</td><td>${duration(item.minutes)}</td></tr>`).join('')}</tbody></table>`:empty('No claims yet','Claims will appear here after customers use the offer.')}</div></section>`;
  $('#free-time-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target);state.settings.freeTimeEnabled=form.get('freeTimeEnabled')==='on';state.settings.freeTimeMinutes=+form.get('freeTimeMinutes');state.settings.freeTimeResetHours=+form.get('freeTimeResetHours');state.settings.requireTerms=form.get('requireTerms')==='on';state.settings.termsTitle=form.get('termsTitle');state.settings.termsBody=form.get('termsBody');await saveSettingsObject(state.settings);toast('Free-time policy and terms saved');renderPage('free-time');};
}

async function renderCoinNodes(){
  [state.coinNodes,state.system]=await Promise.all([api('/coin-nodes'),api('/system')]);
  const online=state.coinNodes.filter(node=>node.online).length;
  $('#app').innerHTML=`${pageHead('Hardware bridge','Coin nodes','Pair ESP32, Arduino WiFi, Raspberry Pi, or Orange Pi coin acceptors without giving them internet access.',`<button class="primary-btn" id="pair-node">＋ Pair coin node</button>`)}
  <section class="stats-grid"><article class="stat-card"><small>Paired nodes</small><strong>${state.coinNodes.length}</strong><footer>Persistent server profiles</footer></article><article class="stat-card"><small>Online</small><strong>${online}</strong><footer>Heartbeat within 45 seconds</footer></article><article class="stat-card"><small>Offline</small><strong>${state.coinNodes.length-online}</strong><footer>Needs power or network check</footer></article><article class="stat-card"><small>Local adapter</small><strong>${state.system.coinSlotMode==='local-socket'?'Ready':'Standby'}</strong><footer>GPIO / serial socket</footer></article></section>
  <section class="card"><div class="card-head"><div><h2>Paired hardware</h2><p>Keys are stored as hashes and shown only once during pairing</p></div></div><div class="node-grid">${state.coinNodes.map(node=>`<article class="node-card"><div class="node-status ${node.online?'online':''}"><i></i>${node.online?'ONLINE':'OFFLINE'}</div><div class="node-icon">◫</div><h3>${esc(node.name)}</h3><code>${esc(node.id)}</code><dl><div><dt>LAN address</dt><dd>${esc(node.clientIp||'Not connected')}</dd></div><div><dt>Firmware</dt><dd>${esc(node.firmware||'Unknown')}</dd></div><div><dt>Last seen</dt><dd>${node.lastSeenAt?dateTime(node.lastSeenAt):'Never'}</dd></div></dl><button class="danger-outline w-100" data-delete-node="${esc(node.id)}">Unpair node</button></article>`).join('')||empty('No coin nodes paired','Pair hardware to generate a device ID and one-time API key.')}</div></section>
  <section class="card safety-card"><b>LAN-only bypass</b><p>Pairing permits the node to call the local ChasselFi API on 10.0.0.1. It does not authenticate the node through openNDS and does not grant public internet access.</p></section>`;
  $('#pair-node').onclick=pairNodeModal;
}

function pairNodeModal(){openModal('Pair a coin node',`<form id="pair-node-form" class="modal-form"><div class="pair-steps"><span>1</span><p><b>Create a server identity</b><small>The key will be visible one time only.</small></p></div><div class="field"><label>Hardware name</label><input name="name" maxlength="64" placeholder="Front counter ESP32" required></div><div class="field"><label>Node ID (optional)</label><input name="nodeId" maxlength="48" placeholder="vendo-front-01"><small>Letters, numbers, dash, and underscore only.</small></div><button class="primary-btn">Generate pairing key</button></form>`);$('#pair-node-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target),result=await api('/coin-nodes',{method:'POST',body:JSON.stringify({name:form.get('name'),nodeId:form.get('nodeId')||null})});$('#modal-body').innerHTML=`<div class="pair-result"><span class="success-orb">✓</span><h3>Node ready to configure</h3><p>Copy these values now. The key cannot be displayed again.</p><label>Node ID</label><code>${esc(result.id)}</code><label>Secret key</label><code class="secret-key">${esc(result.key)}</code><button class="primary-btn" id="copy-node-config">Copy firmware values</button><p class="security-note">${esc(result.note)}</p></div>`;$('#copy-node-config').onclick=async()=>{await navigator.clipboard.writeText(`NODE_ID=${result.id}\nCOIN_NODE_KEY=${result.key}\nSERVER=http://10.0.0.1:2081`);toast('Firmware values copied');};};}

async function renderPortalDesign(){
  state.settings=await api('/settings');const s=state.settings;
  let bannerImage=s.portalBannerImage||'',logoImage=s.portalLogoImage||'';
  const imageMarkup=(value,kind)=>value?`<img src="${esc(value)}" alt="${kind} preview">`:`<span>${kind==='logo'?'C':'No banner image'}</span>`;
  const preview=()=>`<div class="preview-phone ${esc(s.portalTemplate)}" style="--preview-accent:${esc(s.portalAccent)}"><div class="preview-cover" id="preview-cover">${imageMarkup(bannerImage,'banner')}<div class="preview-cover-brand" id="preview-logo">${imageMarkup(logoImage,'logo')}<b>${esc(s.shopName)}</b></div><div><small>${esc(s.portalEyebrow||'FAST • FAIR • LOCAL')}</small><h2>${esc(s.portalHeadline||'Your WiFi. Your time.')}</h2><p>${esc(s.portalMessage)}</p></div></div><div class="preview-connection"><i></i><b>${esc(s.portalStatusLabel||'High-speed connection')}</b><span>Sign in required</span></div><div class="preview-time"><small>TIME REMAINING</small><strong>-- : -- : --</strong></div>${s.portalShowDevice!==false?'<div class="preview-device">ⓘ Device <span>Android · 10.0.0.108</span></div>':''}<button class="preview-rate">${esc(s.portalRatesLabel||'View time rates')} ↗</button><div class="preview-actions">${s.freeTimeEnabled?`<div class="preview-free">✦ <b>${esc(s.portalFreeLabel||'Claim free time')}</b><span>${s.freeTimeMinutes} minutes free</span></div>`:''}${s.paymentMode==='coin'||s.paymentMode==='both'?`<div>₱ <b>${esc(s.portalCoinLabel||'Insert coin')}</b></div>`:''}${s.paymentMode==='voucher'||s.paymentMode==='both'?`<div>V <b>${esc(s.portalVoucherLabel||'Use voucher')}</b></div>`:''}</div></div>`;
  $('#app').innerHTML=`${pageHead('Customer experience','Portal designer','Customize the real openNDS captive popup and the customer portal from one mobile-first design.',`<a class="secondary-btn" href="/portal.html" target="_blank">Open live portal ↗</a>`)}<div class="designer-grid"><section class="card"><form id="portal-design-form" class="modal-form"><div class="field"><label>Portal theme</label><div class="theme-picker"><label><input type="radio" name="portalTemplate" value="aurora" ${s.portalTemplate==='aurora'?'checked':''}><span class="theme-swatch aurora">Aurora</span></label><label><input type="radio" name="portalTemplate" value="midnight" ${s.portalTemplate==='midnight'?'checked':''}><span class="theme-swatch midnight">Midnight</span></label><label><input type="radio" name="portalTemplate" value="sunset" ${s.portalTemplate==='sunset'?'checked':''}><span class="theme-swatch sunset">Sunset</span></label></div></div><div class="form-grid"><div class="field"><label>Accent color</label><input name="portalAccent" type="color" value="${esc(s.portalAccent)}"></div><div class="field"><label>Shop name</label><input name="shopName" value="${esc(s.shopName)}" maxlength="80" required></div></div><div class="field"><label>Small heading</label><input name="portalEyebrow" value="${esc(s.portalEyebrow||'FAST • FAIR • LOCAL')}" maxlength="80"></div><div class="field"><label>Main headline</label><input name="portalHeadline" value="${esc(s.portalHeadline||'Your WiFi. Your time.')}" maxlength="120"></div><div class="field"><label>Welcome message</label><textarea name="portalMessage" maxlength="240" rows="3">${esc(s.portalMessage)}</textarea></div><div class="field"><label>Connection banner text</label><input name="portalStatusLabel" value="${esc(s.portalStatusLabel||'High-speed connection')}" maxlength="80"></div><div class="form-grid"><div class="field"><label>Rates button</label><input name="portalRatesLabel" value="${esc(s.portalRatesLabel||'View time rates')}" maxlength="80"></div><div class="field"><label>Free-time button</label><input name="portalFreeLabel" value="${esc(s.portalFreeLabel||'Claim free time')}" maxlength="80"></div><div class="field"><label>Coin button</label><input name="portalCoinLabel" value="${esc(s.portalCoinLabel||'Insert coin')}" maxlength="80"></div><div class="field"><label>Voucher button</label><input name="portalVoucherLabel" value="${esc(s.portalVoucherLabel||'Use voucher')}" maxlength="80"></div></div>${toggle('portalShowDevice','Show device details','Display the client IP, MAC, gateway, and detected device',s.portalShowDevice!==false)}<div class="portal-image-grid"><div class="field"><label>Banner / hero image</label><input id="portal-banner-file" type="file" accept="image/*"><small>PNG, JPEG, WebP, GIF, or SVG · max 1.8 MB</small><button type="button" class="danger-outline" id="remove-banner">Remove banner</button></div><div class="field"><label>Logo image</label><input id="portal-logo-file" type="file" accept="image/*"><small>Square transparent images work best.</small><button type="button" class="danger-outline" id="remove-logo">Remove logo</button></div></div><button class="primary-btn">Publish portal design</button></form></section><section class="portal-preview-frame" id="portal-preview">${preview()}</section></div>`;
  const refreshPreview=()=>{$('#portal-preview').innerHTML=preview();};
  $('#portal-banner-file').onchange=async event=>{try{bannerImage=await imageDataUrl(event.target.files?.[0]);refreshPreview();}catch(error){toast(error.message,'error');event.target.value='';}};
  $('#portal-logo-file').onchange=async event=>{try{logoImage=await imageDataUrl(event.target.files?.[0]);refreshPreview();}catch(error){toast(error.message,'error');event.target.value='';}};
  $('#remove-banner').onclick=()=>{bannerImage='';refreshPreview();}; $('#remove-logo').onclick=()=>{logoImage='';refreshPreview();};
  $('#portal-design-form').oninput=event=>{const form=new FormData(event.currentTarget);s.portalTemplate=form.get('portalTemplate')||s.portalTemplate;s.portalAccent=form.get('portalAccent')||s.portalAccent;s.shopName=form.get('shopName')||'ChasselFi';s.portalEyebrow=form.get('portalEyebrow');s.portalHeadline=form.get('portalHeadline');s.portalMessage=form.get('portalMessage');s.portalStatusLabel=form.get('portalStatusLabel');s.portalRatesLabel=form.get('portalRatesLabel');s.portalFreeLabel=form.get('portalFreeLabel');s.portalCoinLabel=form.get('portalCoinLabel');s.portalVoucherLabel=form.get('portalVoucherLabel');s.portalShowDevice=form.get('portalShowDevice')==='on';refreshPreview();};
  $('#portal-design-form').onsubmit=async event=>{event.preventDefault();const form=new FormData(event.target);s.portalTemplate=form.get('portalTemplate');s.portalAccent=form.get('portalAccent');s.shopName=form.get('shopName');s.portalEyebrow=form.get('portalEyebrow');s.portalHeadline=form.get('portalHeadline');s.portalMessage=form.get('portalMessage');s.portalStatusLabel=form.get('portalStatusLabel');s.portalRatesLabel=form.get('portalRatesLabel');s.portalFreeLabel=form.get('portalFreeLabel');s.portalCoinLabel=form.get('portalCoinLabel');s.portalVoucherLabel=form.get('portalVoucherLabel');s.portalShowDevice=form.get('portalShowDevice')==='on';s.portalBannerImage=bannerImage;s.portalLogoImage=logoImage;await saveSettingsObject(s);toast('Customer portal design published');renderPage('portal-design');};
}

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
  <section class="setting-card payment-setting"><h2>Customer access methods</h2><p>Enable any combination, including Free-Time-only.</p><div class="access-method-switches">${toggle('enableVoucher','Voucher mode','Show code entry and accept generated vouchers',s.paymentMode==='voucher'||s.paymentMode==='both')}${toggle('enableCoin','Coin mode','Show packages and accept confirmed hardware pulses',s.paymentMode==='coin'||s.paymentMode==='both')}${toggle('freeTimeEnabled','Free time','Emphasize the complimentary claim on the captive portal',s.freeTimeEnabled)}</div><div class="field" style="margin-top:14px"><label>Value per hardware pulse (peso)</label><input name="coinPulseValue" type="number" min="1" max="100" value="${s.coinPulseValue||1}"><small>Use 1 for a standard ₱1 pulse acceptor.</small></div>${toggle('autoPause','Brownout auto-pause','Preserve remaining time after interruption',s.autoPause)}</section>
  <section class="setting-card"><h2>Speed limits</h2><p>Default per-user bandwidth</p><div class="form-grid"><div class="field"><label>Download (Mbps)</label><input name="downloadLimitMbps" type="number" min="1" value="${s.downloadLimitMbps}"></div><div class="field"><label>Upload (Mbps)</label><input name="uploadLimitMbps" type="number" min="1" value="${s.uploadLimitMbps}"></div></div></section>
  <section class="setting-card"><h2>Maintenance</h2><p>Scheduled service behavior</p>${toggle('maintenanceSchedule','Scheduled maintenance','Enable the daily maintenance window',s.maintenanceSchedule)}<div class="field" style="margin-top:14px"><label>Window</label><input value="03:00 Asia/Manila" disabled></div></section></div><div class="save-bar"><button class="primary-btn">Save all changes</button></div></form>`;
  $(`[name="timezone"]`).value=s.timezone;
  $('#settings-form').onsubmit=saveSettings;
  $('#download-backup').onclick=downloadBackup;
  $('#restore-backup').onclick=()=>$('#backup-file').click();
  $('#backup-file').onchange=restoreBackup;
}
function toggle(name,title,desc,on){return `<label class="toggle-row"><span><strong>${title}</strong><small>${desc}</small></span><span class="switch"><input type="checkbox" name="${name}" ${on?'checked':''}><i></i></span></label>`;}
async function saveSettings(e){e.preventDefault();const f=new FormData(e.target),voucher=f.get('enableVoucher')==='on',coin=f.get('enableCoin')==='on',mode=voucher&&coin?'both':voucher?'voucher':coin?'coin':'none',body={...state.settings,shopName:f.get('shopName'),timezone:f.get('timezone'),currency:f.get('currency'),portalMessage:f.get('portalMessage'),paymentMode:mode,coinPulseValue:+f.get('coinPulseValue'),buyTime:coin,vouchers:voucher,freeTimeEnabled:f.get('freeTimeEnabled')==='on',autoPause:f.get('autoPause')==='on',downloadLimitMbps:+f.get('downloadLimitMbps'),uploadLimitMbps:+f.get('uploadLimitMbps'),maintenanceSchedule:f.get('maintenanceSchedule')==='on'};await saveSettingsObject(body);toast('Customer access methods and system settings saved');}
async function downloadBackup(){const backup=await api('/backup');const link=document.createElement('a');link.href=URL.createObjectURL(new Blob([JSON.stringify(backup,null,2)],{type:'application/json'}));link.download=`chasselfi-backup-${new Date().toISOString().slice(0,10)}.json`;link.click();URL.revokeObjectURL(link.href);toast('Backup downloaded');}
async function restoreBackup(event){const file=event.target.files?.[0];if(!file)return;try{const backup=JSON.parse(await file.text());await api('/backup/restore',{method:'POST',body:JSON.stringify(backup)});toast('Backup restored. Reloading…');setTimeout(()=>location.reload(),600);}catch(error){toast(error.message,'error');}event.target.value='';}

async function renderPage(forced) {
  clearInterval(bandwidthTimer);
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
    else if (page === 'free-time') await renderFreeTime();
    else if (page === 'coin-nodes') await renderCoinNodes();
    else if (page === 'network') { if(!state.settings) state.settings=await api('/settings'); await renderNetwork(); }
    else if (page === 'tools') await renderTools();
    else if (page === 'portal-design') await renderPortalDesign();
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
  const editSession=event.target.closest('[data-edit-session]');if(editSession)return sessionModal(state.sessions.find(item=>item.id===editSession.dataset.editSession));
  const editRate=event.target.closest('[data-edit-rate]'); if(editRate)return rateModal(state.rates.find(r=>r.id===editRate.dataset.editRate));
  const deleteRate=event.target.closest('[data-delete-rate]'); if(deleteRate&&confirm('Delete this timer rate?')){await api(`/rates/${deleteRate.dataset.deleteRate}`,{method:'DELETE'});toast('Rate deleted');return renderPage('rates');}
  const delVoucher=event.target.closest('[data-delete-voucher]');if(delVoucher&&confirm('Delete this voucher?')){await api(`/vouchers/${delVoucher.dataset.deleteVoucher}`,{method:'DELETE'});toast('Voucher deleted');return renderPage('vouchers');}
  const delNode=event.target.closest('[data-delete-node]');if(delNode&&confirm('Unpair this coin node? Its current key will stop working immediately.')){await api(`/coin-nodes/${delNode.dataset.deleteNode}`,{method:'DELETE'});toast('Coin node unpaired');return renderPage('coin-nodes');}
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
