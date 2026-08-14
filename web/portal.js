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
  $('.portal-status strong').textContent = settings.portalStatusLabel || 'High-speed connection';
  $('.portal-status small').textContent = 'Access required';
  $('.pulse-dot').classList.add('is-offline');
  $('#remaining-summary strong').textContent = '-- : -- : --';
  $('#remaining-summary span').textContent = settings.freeTimeEnabled ? 'Free time may be available on the sign-in screen' : 'Connect to start your session';
}

function renderStatus(){
  clearInterval(coinPollTimer);
  if (portalStatus.connected){
    showConnected(portalStatus.session.remainingSeconds, 'Internet access is active', portalStatus.session);
    return;
  }
  showAccessRequired();
  const coinLabel=settings.portalCoinLabel||'Insert coin',voucherLabel=settings.portalVoucherLabel||'Use voucher',freeLabel=settings.portalFreeLabel||'Claim free time';
  const instruction = allowsCoin() && allowsVoucher() ? `${coinLabel} or ${voucherLabel}` : allowsCoin() ? coinLabel : allowsVoucher() ? voucherLabel : settings.freeTimeEnabled ? freeLabel : 'Ask the WiFi operator';
  const freeOffer=settings.freeTimeEnabled?`<section class="portal-method free-method direct-free-offer"><div class="free-ribbon">FREE ACCESS</div><div class="method-icon">✦</div><div><span class="eyebrow">COMPLIMENTARY SESSION</span><h2>${esc(settings.portalFreeLabel||'Claim free time')}</h2><p><strong>${duration(settings.freeTimeMinutes||15)}</strong> for each eligible device.</p></div><button class="btn free-claim-btn portal-cta" id="claim-free-time">Read terms & claim <span>→</span></button></section>`:'';
  $('#portal-panel').innerHTML = `${freeOffer}<div class="portal-welcome"><span class="eyebrow">CONNECT TO INTERNET</span><h2>${esc(instruction)}</h2><p>Internet opens only after the server confirms the selected access method.</p><button class="btn secondary-btn w-100 mt-3" id="show-rates">${esc(settings.portalRatesLabel||'View time rates')}</button></div>`;
  $('#show-rates').onclick=()=>$('#portal-rates-dialog').showModal();
  $('#claim-free-time')?.addEventListener('click',event=>{
    if(settings.requireTerms) $('#portal-free-dialog').showModal();
    else claimFreeTime(false,event.currentTarget);
  });
}

async function claimFreeTime(acceptedTerms,trigger){
  const button=trigger||$('#portal-free-confirm');
  if(button){button.disabled=true;button.textContent='Confirming this device...';}
  try{
    const result=await api('/portal/free',{method:'POST',body:JSON.stringify({deviceKey,acceptedTerms})});
    $('#portal-free-dialog').close();
    portalStatus={connected:true,session:result.session};
    showConnected(result.session.remainingSeconds,'Free time claimed - internet is active',result.session);
  }catch(error){if($('#portal-free-dialog').open)$('#portal-free-dialog').close();if(button){button.disabled=false;button.textContent='Accept and claim free time →';}showPortalError(error.message);}
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
  $('.portal-status strong').textContent = settings.portalStatusLabel || 'High-speed connection'; $('.portal-status small').textContent = message; $('.pulse-dot').classList.remove('is-offline');
  $('#remaining-summary strong').textContent = duration(Math.ceil(seconds/60));
  $('#remaining-summary span').textContent = session.status==='paused' ? 'Session safely paused' : 'Internet access active';
  let localRemaining = Math.max(0,Math.round(seconds));
  const addButtons = `${allowsCoin()?'<button class="btn secondary-btn" data-add-coin>Add coins</button>':''}${allowsVoucher()?'<button class="btn secondary-btn" data-add-voucher>Add voucher</button>':''}`;
  const paused=session.status==='paused';
  const canPause=paused||(session.customerPauseEnabled??settings.customerPauseEnabled);
  const pauseButton=canPause?`<button class="btn ${paused?'primary-btn':'secondary-btn'} w-100 mb-2" id="customer-session-toggle">${paused?'Resume internet':'Pause my time'}</button>`:'';
  const pauseProgress=!paused&&canPause?`<small class="pause-policy">Pauses used ${session.pauseCount||0} of ${session.pauseLimitCount??settings.pauseLimitCount??3}</small>`:'';
  $('#portal-panel').innerHTML = `<div class="portal-session"><span class="eyebrow">${paused?'SESSION PAUSED':'SESSION ACTIVE'}</span><h2>${duration(Math.ceil(seconds/60))}</h2><p>${paused?'time is safely preserved':'remaining on this device'}</p><div class="portal-speed"><span><small>DOWNLOAD LIMIT</small><b>${session.downloadMbps || '-'} Mbps</b></span><span><small>UPLOAD LIMIT</small><b>${session.uploadMbps || '-'} Mbps</b></span></div>${pauseButton}${pauseProgress}<div class="portal-add-actions">${addButtons}</div></div>`;
  if(canPause) $('#customer-session-toggle').onclick=async event=>{event.target.disabled=true;try{await api(`/portal/session/${paused?'resume':'pause'}`,{method:'POST',body:'{}'});const result=await api('/portal/status');portalStatus=result;showConnected(result.session.remainingSeconds,paused?'Internet resumed':'Time paused',result.session);}catch(error){event.target.disabled=false;showPortalError(error.message);}};
  $('[data-add-coin]')?.addEventListener('click',() => activateTab('coin'));
  $('[data-add-voucher]')?.addEventListener('click',() => activateTab('voucher'));
  const renderRemaining = remaining => {
    localRemaining = Math.max(0,Math.round(remaining));
    $('#portal-panel h2').textContent = duration(Math.ceil(localRemaining/60));
    $('#remaining-summary strong').textContent = duration(Math.ceil(localRemaining/60));
    const warningMinutes=session.lowTimeWarningMinutes??settings.lowTimeWarningMinutes??5;
    $('#remaining-summary').classList.toggle('low-time',warningMinutes>0&&localRemaining<=warningMinutes*60);
    if (localRemaining === 0){ $('.portal-status small').textContent = 'Session expired'; $('.pulse-dot').style.background = 'var(--red)'; clearInterval(heartbeatTimer); clearInterval(countdownTimer); }
  };
  if(!paused) countdownTimer = setInterval(() => renderRemaining(localRemaining - 1),1000);
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
  $('#portal-eyebrow').textContent = settings.portalEyebrow || 'FAST • FAIR • LOCAL';
  $('#portal-headline').textContent = settings.portalHeadline || 'Your WiFi. Your time.';
  $('#portal-status-label').textContent = settings.portalStatusLabel || 'High-speed connection';
  $('#show-rates-top').firstChild.textContent = `${settings.portalRatesLabel || 'View time rates'} `;
  $('[data-portal-tab="coin"]').textContent = settings.portalCoinLabel || 'Insert coin';
  $('[data-portal-tab="voucher"]').textContent = settings.portalVoucherLabel || 'Voucher';
  $('#device-ip').textContent = portalStatus.clientIp || 'Local client';
  $('#device-mac').textContent = portalStatus.clientMac || 'Waiting for gateway';
  $('#portal-device').hidden = settings.portalShowDevice === false;
  if (settings.portalBannerImage){ $('#portal-banner').src=settings.portalBannerImage; $('#portal-banner').hidden=false; }
  if (settings.portalLogoImage){ $('#portal-logo').src=settings.portalLogoImage; $('#portal-logo').hidden=false; $('#portal-logo-fallback').hidden=true; }
  document.body.classList.add(`template-${settings.portalTemplate||'aurora'}`);
  document.body.style.setProperty('--portal-accent',settings.portalAccent||'#28d17c');
  $('#portal-modal-rates').innerHTML=rates.map(rate=>`<article class="fas-rate"><strong>${money(rate.price)}</strong><span>${duration(rate.minutes)}</span><small>↓ ${rate.downloadMbps} Mbps · ↑ ${rate.uploadMbps} Mbps · ${esc(rate.label)}</small></article>`).join('');
  $('#portal-free-title').textContent=settings.termsTitle||'Fair use';
  $('#portal-free-terms').textContent=settings.termsBody||'Use this shared connection fairly.';
  $('[data-portal-tab="coin"]').hidden = !allowsCoin();
  $('[data-portal-tab="voucher"]').hidden = !allowsVoucher();
  $('.portal-tabs').style.gridTemplateColumns = `repeat(${1 + Number(allowsCoin()) + Number(allowsVoucher())},1fr)`;
  const requested = location.hash.slice(1);
  activateTab(requested === 'coin' && allowsCoin() ? 'coin' : requested === 'voucher' && allowsVoucher() ? 'voucher' : 'status');
}).catch(error => { $('#portal-panel').innerHTML = `<div class="empty-state"><b>Portal temporarily unavailable</b><span>${esc(error.message)}</span></div>`; });
$('#show-rates-top').onclick=()=>$('#portal-rates-dialog').showModal();
document.querySelector('[data-close-rates]').onclick=()=>$('#portal-rates-dialog').close();
$('#portal-rates-dialog').onclick=event=>{if(event.target===$('#portal-rates-dialog'))event.target.close();};
document.querySelector('[data-close-free]').onclick=()=>$('#portal-free-dialog').close();
$('#portal-free-confirm').onclick=event=>claimFreeTime(true,event.currentTarget);
$('#portal-free-dialog').onclick=event=>{if(event.target===$('#portal-free-dialog'))event.target.close();};
