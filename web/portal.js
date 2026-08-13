const $ = (selector, root = document) => root.querySelector(selector);
const esc = value => String(value ?? '').replace(/[&<>'"]/g, character => ({'&':'&amp;','<':'&lt;','>':'&gt;',"'":'&#39;','"':'&quot;'}[character]));
const api = async (path, options = {}) => {
  const response = await fetch(`/api${path}`, {headers:{'Content-Type':'application/json'}, ...options});
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || 'Could not complete this request');
  return data;
};
const money = number => new Intl.NumberFormat('en-PH',{style:'currency',currency:'PHP',maximumFractionDigits:0}).format(number);
const duration = minutes => minutes >= 1440 ? `${Math.floor(minutes/1440)} day${minutes >= 2880 ? 's' : ''}` : minutes >= 60 ? `${Math.floor(minutes/60)} hr${Math.floor(minutes/60)>1?'s':''}${minutes%60?` ${minutes%60} min`:''}` : `${minutes} min`;

let rates = [];
let settings = {paymentMode:'voucher'};
let portalStatus = {connected:false};
let coinClaim = JSON.parse(sessionStorage.getItem('chasselfiCoinClaim') || 'null');
let coinPollTimer = null;
let heartbeatTimer = null;
let countdownTimer = null;
const deviceKey = localStorage.getItem('chasselfiDeviceKey') || (() => {
  const value = `device-${crypto.randomUUID?.() || Math.random().toString(36).slice(2)}`;
  localStorage.setItem('chasselfiDeviceKey', value);
  return value;
})();

function allowsCoin(){ return settings.paymentMode === 'coin' || settings.paymentMode === 'both'; }
function allowsVoucher(){ return settings.paymentMode === 'voucher' || settings.paymentMode === 'both'; }
function clock(){ $('#portal-clock').textContent = new Date().toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'}); }
setInterval(clock,1000); clock();
$('#device-id').textContent = /Android/i.test(navigator.userAgent) ? 'Android device' : /iPhone/i.test(navigator.userAgent) ? 'iPhone' : 'Connected device';

function showPortalError(message){
  let box = $('.portal-alert');
  if (!box){ box = document.createElement('div'); box.className = 'portal-alert'; $('#portal-panel').prepend(box); }
  box.textContent = message;
}

function showAccessRequired(){
  $('.portal-status strong').textContent = 'Access required';
  $('.portal-status small').textContent = 'THIS DEVICE';
  $('.pulse-dot').classList.add('is-offline');
}

function renderStatus(){
  clearInterval(coinPollTimer);
  if (portalStatus.connected){
    showConnected(portalStatus.session.remainingSeconds, 'Internet access is active', portalStatus.session);
    return;
  }
  showAccessRequired();
  const instruction = allowsCoin() && allowsVoucher() ? 'Insert coins or use a voucher' : allowsCoin() ? 'Insert coins to start' : 'Use a voucher to start';
  $('#portal-panel').innerHTML = `<div class="portal-welcome"><span class="eyebrow">CONNECT TO INTERNET</span><h2>${instruction}</h2><p>Choose an available payment method. Internet opens only after the server confirms the payment.</p></div><div class="portal-rates">${rates.map(rate => `<article class="portal-rate"><b>${money(rate.price)}</b><span>${duration(rate.minutes)}</span><small>${rate.downloadMbps} Mbps &middot; ${esc(rate.label)}</small></article>`).join('')}</div>`;
}

function renderVoucher(){
  clearInterval(coinPollTimer);
  if (!allowsVoucher()){ activateTab(allowsCoin() ? 'coin' : 'status'); return; }
  showAccessRequired();
  $('#portal-panel').innerHTML = `<form class="voucher-entry needs-validation" id="redeem-form"><div class="field"><label class="form-label">Enter your 8-character voucher code</label><input class="form-control form-control-lg" name="code" maxlength="8" minlength="8" autocomplete="one-time-code" placeholder="AB12CD34" required></div><button class="btn primary-btn portal-cta">Connect with voucher</button><small class="text-center">Each code may only be used once.</small></form>`;
  $('#redeem-form').onsubmit = async event => {
    event.preventDefault();
    const button = event.target.querySelector('button');
    button.disabled = true; button.textContent = 'Checking code...';
    try {
      const result = await api('/vouchers/redeem',{method:'POST',body:JSON.stringify({code:new FormData(event.target).get('code'),deviceKey})});
      portalStatus = {connected:true,session:result.session};
      showConnected(result.session.remainingSeconds,'Voucher accepted - time added',result.session);
    } catch (error){ button.disabled = false; button.textContent = 'Connect with voucher'; showPortalError(error.message); }
  };
}

function renderCoin(){
  if (!allowsCoin()){ activateTab(allowsVoucher() ? 'voucher' : 'status'); return; }
  showAccessRequired();
  if (coinClaim){ renderCoinProgress(coinClaim); return; }
  $('#portal-panel').innerHTML = `<div class="coin-mode"><div class="portal-welcome"><span class="eyebrow">PHYSICAL COIN MODE</span><h2>Choose your time</h2><p>Select a package first. Wait for the coin node to show READY before inserting coins.</p></div><div class="portal-rates">${rates.map(rate => `<button class="portal-rate coin-rate" data-rate-id="${rate.id}"><b>${money(rate.price)}</b><span>${duration(rate.minutes)}</span><small>${rate.downloadMbps} Mbps &middot; ${esc(rate.label)}</small></button>`).join('')}</div><div class="coin-safety"><b>Coins are accepted only during an active claim.</b><span>Do not insert coins while the hardware indicator is off.</span></div></div>`;
  document.querySelectorAll('[data-rate-id]').forEach(button => button.onclick = () => startCoinClaim(button.dataset.rateId, button));
}

async function startCoinClaim(rateId, button){
  document.querySelectorAll('[data-rate-id]').forEach(item => item.disabled = true);
  if (button) button.classList.add('selected');
  try {
    coinClaim = await api('/portal/purchase',{method:'POST',body:JSON.stringify({rateId,deviceKey})});
    sessionStorage.setItem('chasselfiCoinClaim', JSON.stringify(coinClaim));
    renderCoinProgress(coinClaim);
  } catch (error){
    document.querySelectorAll('[data-rate-id]').forEach(item => item.disabled = false);
    showPortalError(error.message);
  }
}

function renderCoinProgress(claim){
  clearInterval(coinPollTimer);
  const percent = Math.min(100, Math.round((claim.insertedPesos / claim.requiredPesos) * 100));
  $('#portal-panel').innerHTML = `<div class="coin-progress"><span class="coin-ready"><i></i> COIN SLOT READY</span><h2>Insert ${money(claim.remainingPesos)}</h2><p>Package: ${duration(claim.rate.minutes)} at ${money(claim.requiredPesos)}</p><div class="coin-meter"><i style="width:${percent}%"></i></div><div class="coin-totals"><span><small>INSERTED</small><b>${money(claim.insertedPesos)}</b></span><span><small>REQUIRED</small><b>${money(claim.requiredPesos)}</b></span></div><small class="coin-warning">Keep this page open. Inserted coins cannot be cancelled or refunded automatically.</small>${claim.insertedPesos === 0 ? '<button class="btn secondary-btn w-100" data-cancel-coin>Cancel</button>' : ''}</div>`;
  $('[data-cancel-coin]')?.addEventListener('click', cancelCoinClaim);
  coinPollTimer = setInterval(refreshCoinClaim, 1000);
}

async function refreshCoinClaim(){
  if (!coinClaim) return;
  try {
    const result = await api(`/portal/coin/status?claimId=${encodeURIComponent(coinClaim.claimId)}`);
    if (result.status === 'completed'){
      clearInterval(coinPollTimer);
      sessionStorage.removeItem('chasselfiCoinClaim'); coinClaim = null;
      portalStatus = {connected:true,session:result.session};
      showConnected(result.session.remainingSeconds, result.gatewayWarning ? 'Payment confirmed - reconnecting gateway' : 'Coins accepted - internet is active', result.session);
      return;
    }
    if (result.insertedPesos !== coinClaim.insertedPesos){ coinClaim = result; sessionStorage.setItem('chasselfiCoinClaim',JSON.stringify(result)); renderCoinProgress(result); }
  } catch (error){ clearInterval(coinPollTimer); sessionStorage.removeItem('chasselfiCoinClaim'); coinClaim = null; showPortalError(error.message); }
}

async function cancelCoinClaim(){
  try { await api('/portal/coin/cancel',{method:'POST',body:JSON.stringify({claimId:coinClaim.claimId})}); coinClaim = null; sessionStorage.removeItem('chasselfiCoinClaim'); renderCoin(); }
  catch (error){ showPortalError(error.message); }
}

function showConnected(seconds, message, session){
  clearInterval(heartbeatTimer); clearInterval(countdownTimer); clearInterval(coinPollTimer);
  $('.portal-status strong').textContent = 'Connected'; $('.portal-status small').textContent = message; $('.pulse-dot').classList.remove('is-offline');
  let localRemaining = Math.max(0,Math.round(seconds));
  const addButtons = `${allowsCoin()?'<button class="btn secondary-btn" data-add-coin>Add coins</button>':''}${allowsVoucher()?'<button class="btn secondary-btn" data-add-voucher>Add voucher</button>':''}`;
  $('#portal-panel').innerHTML = `<div class="portal-session"><span class="eyebrow">SESSION ACTIVE</span><h2>${duration(Math.ceil(seconds/60))}</h2><p>remaining on this device</p><div class="portal-speed"><span><small>DOWNLOAD LIMIT</small><b>${session.downloadMbps || '-'} Mbps</b></span><span><small>UPLOAD LIMIT</small><b>${session.uploadMbps || '-'} Mbps</b></span></div><div class="portal-add-actions">${addButtons}</div></div>`;
  $('[data-add-coin]')?.addEventListener('click',() => activateTab('coin'));
  $('[data-add-voucher]')?.addEventListener('click',() => activateTab('voucher'));
  const renderRemaining = remaining => {
    localRemaining = Math.max(0,Math.round(remaining));
    $('#portal-panel h2').textContent = duration(Math.ceil(localRemaining/60));
    if (localRemaining === 0){ $('.portal-status strong').textContent = 'Expired'; $('.pulse-dot').style.background = 'var(--red)'; clearInterval(heartbeatTimer); clearInterval(countdownTimer); }
  };
  countdownTimer = setInterval(() => renderRemaining(localRemaining - 1),1000);
  heartbeatTimer = setInterval(async () => { try { const result = await api('/portal/status'); if (!result.connected){ clearInterval(heartbeatTimer); clearInterval(countdownTimer); portalStatus=result; renderStatus(); return; } renderRemaining(result.session.remainingSeconds); } catch {} },30000);
}

function activateTab(name){
  document.querySelectorAll('[data-portal-tab]').forEach(tab => tab.classList.toggle('active',tab.dataset.portalTab === name));
  if (name === 'coin') renderCoin(); else if (name === 'voucher') renderVoucher(); else renderStatus();
  if (name !== 'status') history.replaceState(null,'',`#${name}`);
}

document.querySelectorAll('[data-portal-tab]').forEach(button => button.onclick = () => activateTab(button.dataset.portalTab));
Promise.all([api('/rates'),api('/settings'),api('/portal/status')]).then(([rateList,configured,status]) => {
  rates = rateList.filter(rate => rate.active);
  settings = configured;
  portalStatus = status;
  $('#portal-shop').textContent = settings.shopName;
  $('#portal-message').textContent = settings.portalMessage;
  $('[data-portal-tab="coin"]').hidden = !allowsCoin();
  $('[data-portal-tab="voucher"]').hidden = !allowsVoucher();
  $('.portal-tabs').style.gridTemplateColumns = `repeat(${1 + Number(allowsCoin()) + Number(allowsVoucher())},1fr)`;
  const requested = location.hash.slice(1);
  activateTab(requested === 'coin' && allowsCoin() ? 'coin' : requested === 'voucher' && allowsVoucher() ? 'voucher' : 'status');
}).catch(error => { $('#portal-panel').innerHTML = `<div class="empty-state"><b>Portal temporarily unavailable</b><span>${esc(error.message)}</span></div>`; });
