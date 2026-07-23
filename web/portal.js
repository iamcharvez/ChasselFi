const $ = (selector, root = document) => root.querySelector(selector);
const api = async (path, options={}) => {
  const response = await fetch(`/api${path}`, { headers:{'Content-Type':'application/json'}, ...options });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || 'Could not complete this request');
  return data;
};
const money = n => new Intl.NumberFormat('en-PH',{style:'currency',currency:'PHP',maximumFractionDigits:0}).format(n);
const duration = m => m>=1440?`${Math.floor(m/1440)} day`:m>=60?`${Math.floor(m/60)} hr${Math.floor(m/60)>1?'s':''}${m%60?` ${m%60} min`:''}`:`${m} min`;
let rates=[], selected=null;
const deviceKey = localStorage.getItem('chasselfiDeviceKey') || (()=>{const value=`device-${crypto.randomUUID?.()||Math.random().toString(36).slice(2)}`;localStorage.setItem('chasselfiDeviceKey',value);return value;})();
let activeSession = JSON.parse(localStorage.getItem('chasselfiSession')||'null');
let heartbeatTimer = null;

function clock(){ $('#portal-clock').textContent=new Date().toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'}); }
setInterval(clock,1000);clock();
$('#device-id').textContent = /Android/i.test(navigator.userAgent)?'Android device':/iPhone/i.test(navigator.userAgent)?'iPhone':'Connected device';

function renderTime(){
  $('#portal-panel').innerHTML=`<div class="portal-rates">${rates.map((r,i)=>`<button class="btn portal-rate ${selected?.id===r.id?'selected':''}" data-rate="${r.id}"><b>${money(r.price)}</b><span>${duration(r.minutes)}</span><small>${r.downloadMbps} Mbps · ${r.label}</small></button>`).join('')}</div><button class="btn primary-btn portal-cta" id="insert-coin" ${selected?'':'disabled'}>${selected?`Insert ${money(selected.price)} in the coin slot`:'Choose a time package'}</button>`;
  document.querySelectorAll('[data-rate]').forEach(btn=>btn.onclick=()=>{selected=rates.find(r=>r.id===btn.dataset.rate);renderTime();});
  $('#insert-coin').onclick=async()=>{if(!selected)return;$('#insert-coin').textContent='Waiting for coin pulse…';await new Promise(r=>setTimeout(r,900));const result=await api('/portal/purchase',{method:'POST',body:JSON.stringify({rateId:selected.id,deviceKey})});showConnected(result.session.remainingSeconds,`Payment received · ${money(selected.price)}`,result.session);};
}
function renderVoucher(){
  $('#portal-panel').innerHTML=`<form class="voucher-entry needs-validation" id="redeem-form"><div class="field"><label class="form-label">Enter your 8-character voucher code</label><input class="form-control form-control-lg" name="code" maxlength="8" minlength="8" autocomplete="one-time-code" placeholder="AB12CD34" required></div><button class="btn primary-btn portal-cta">Connect with voucher</button><small class="text-center">Each code may only be used once.</small></form>`;
  $('#redeem-form').onsubmit=async e=>{e.preventDefault();const btn=e.target.querySelector('button');btn.textContent='Checking code…';try{const result=await api('/vouchers/redeem',{method:'POST',body:JSON.stringify({code:new FormData(e.target).get('code'),deviceKey})});showConnected(result.session.remainingSeconds,'Voucher accepted',result.session);}catch(error){btn.textContent='Connect with voucher';alert(error.message);}};
}
function showConnected(seconds,message,session){
  activeSession=session||activeSession;
  if(activeSession)localStorage.setItem('chasselfiSession',JSON.stringify(activeSession));
  clearInterval(heartbeatTimer);
  $('.portal-status strong').textContent='Connected'; $('.portal-status small').textContent=message; $('.pulse-dot').style.background='var(--green)';
  const renderRemaining=remaining=>{const safe=Math.max(0,Math.round(remaining));$('#portal-panel h2').textContent=duration(Math.ceil(safe/60));if(safe===0){$('.portal-status strong').textContent='Expired';$('.pulse-dot').style.background='var(--red)';clearInterval(heartbeatTimer);}};
  $('#portal-panel').innerHTML=`<div class="text-center p-3"><span class="eyebrow">SESSION ACTIVE</span><h2 class="display-6 fw-bold mt-3 mb-1">${duration(Math.ceil(seconds/60))}</h2><p class="text-secondary mt-0">secured to this device</p><div class="progress my-4" style="height:8px"><div class="progress-bar bg-success" style="width:100%"></div></div><button class="btn secondary-btn" onclick="location.reload()">Back to portal</button></div>`;
  heartbeatTimer=setInterval(async()=>{if(!activeSession)return;try{const result=await api('/session/heartbeat',{method:'POST',body:JSON.stringify({sessionId:activeSession.id,token:activeSession.token,deviceKey:activeSession.deviceKey})});activeSession.remainingSeconds=result.remainingSeconds;localStorage.setItem('chasselfiSession',JSON.stringify(activeSession));renderRemaining(result.remainingSeconds);}catch(error){clearInterval(heartbeatTimer);}},30000);
}

document.querySelectorAll('[data-portal-tab]').forEach(btn=>btn.onclick=()=>{document.querySelectorAll('[data-portal-tab]').forEach(x=>x.classList.toggle('active',x===btn));btn.dataset.portalTab==='time'?renderTime():renderVoucher();});

Promise.all([api('/rates'),api('/settings')]).then(([r,s])=>{rates=r.filter(x=>x.active);selected=rates[0];$('#portal-shop').textContent=s.shopName;$('#portal-message').textContent=s.portalMessage;renderTime();}).catch(error=>{$('#portal-panel').innerHTML=`<div class="empty-state"><b>Portal temporarily unavailable</b>${error.message}</div>`;});
