const app={containers:[],range:3600000,detailId:null,timer:null};
const $=id=>document.getElementById(id);
const esc=value=>String(value??"").replace(/[&<>'"]/g,char=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;",'"':"&quot;"}[char]));
const fmtPct=value=>value==null?"—":`${value.toFixed(value>=10?1:2)}%`;
const fmtBytes=value=>{if(value==null)return"—";const units=["B","KiB","MiB","GiB","TiB"];let index=0;let n=Number(value);while(Math.abs(n)>=1024&&index<units.length-1){n/=1024;index++}return`${n.toFixed(index?1:0)} ${units[index]}`};
const fmtRate=value=>value==null?"—":`${fmtBytes(value)}/s`;
const fmtTime=value=>value?new Date(value).toLocaleString("zh-CN",{hour12:false}):"—";

async function api(path){const response=await fetch(path,{headers:{Accept:"application/json"}});const body=await response.json().catch(()=>({}));if(!response.ok)throw new Error(body.message||`请求失败 (${response.status})`);return body}
function notify(message){const toast=$("toast");toast.textContent=message;toast.classList.remove("hidden");clearTimeout(toast.timer);toast.timer=setTimeout(()=>toast.classList.add("hidden"),5000)}

async function refreshDashboard(){
  try{
    const [list,status]=await Promise.all([api("/api/v1/containers"),api("/api/v1/system/status")]);
    app.containers=list.containers;renderStatus(status);renderSummary();renderRows();
    $("last-updated").textContent=`最后刷新 ${fmtTime(list.generated_at_ms)}`;
  }catch(error){setStatus(false,error.message);notify(error.message)}
}
function renderStatus(status){setStatus(status.collector.docker_connected,status.collector.docker_connected?`采集正常 · ${status.collector_interval_seconds} 秒`:(status.collector.last_error||"Docker 暂时不可用"));$("running-count").textContent=status.running_containers;$("stopped-count").textContent=status.stopped_containers}
function setStatus(ok,text){$("status-dot").className=`status-dot ${ok?"ok":"error"}`;$("status-text").textContent=text}
function renderSummary(){const running=app.containers.filter(item=>item.state==="running");const cpu=running.reduce((sum,item)=>sum+(item.cpu_percent||0),0);const memory=running.reduce((sum,item)=>sum+(item.memory_working_set_bytes||0),0);$("cpu-total").textContent=fmtPct(cpu);$("memory-total").textContent=fmtBytes(memory)}
function renderRows(){
  const query=$("search-input").value.trim().toLowerCase();const rows=app.containers.filter(item=>`${item.name} ${item.image} ${item.docker_id}`.toLowerCase().includes(query));
  $("container-rows").innerHTML=rows.length?rows.map(item=>`<tr data-id="${esc(item.docker_id)}"><td><div class="container-name"><strong>${esc(item.name)}</strong><small>${esc(item.image)} · ${esc(item.docker_id.slice(0,12))}</small></div></td><td><span class="state-pill ${esc(item.state)}">${esc(item.state)}</span></td><td class="metric-value">${fmtPct(item.cpu_percent)}</td><td class="metric-value">${fmtBytes(item.memory_working_set_bytes)} <small>${fmtPct(item.memory_percent)}</small></td><td class="metric-value">${fmtBytes(item.network_rx_bytes)} / ${fmtBytes(item.network_tx_bytes)}</td><td class="metric-value">${fmtBytes(item.block_read_bytes)} / ${fmtBytes(item.block_write_bytes)}</td><td class="metric-value">${item.pids??"—"}</td></tr>`).join(""):`<tr><td colspan="7" class="empty">${query?"没有匹配的容器":"尚未采集到容器，请检查 Docker 连接"}</td></tr>`;
  document.querySelectorAll("#container-rows tr[data-id]").forEach(row=>row.addEventListener("click",()=>navigate(`/containers/${row.dataset.id}`)));
}

function navigate(path){history.pushState({},"",path);route()}
function route(){
  const match=location.pathname.match(/^\/containers\/([a-f0-9]+)$/i);clearInterval(app.timer);
  if(match){app.detailId=match[1];$("dashboard-page").classList.add("hidden");$("detail-page").classList.remove("hidden");refreshDetail();app.timer=setInterval(refreshDetail,15000)}
  else{app.detailId=null;$("detail-page").classList.add("hidden");$("dashboard-page").classList.remove("hidden");refreshDashboard();app.timer=setInterval(refreshDashboard,15000)}
}
async function refreshDetail(){
  const to=Date.now(),from=to-app.range,id=app.detailId,params=`from=${from}&to=${to}`;
  try{
    const [metrics,events]=await Promise.all([api(`/api/v1/containers/${id}/metrics?${params}`),api(`/api/v1/containers/${id}/events?${params}`)]);
    renderDetail(metrics,events);setStatus(true,"指标页面已刷新")
  }catch(error){notify(error.message);setStatus(false,error.message)}
}
function renderDetail(response,eventResponse){
  const item=response.container,points=response.points;document.title=`${item.name} · Pulse`;$("detail-name").textContent=item.name;$("detail-state").textContent=item.state;$("detail-state").className=`state-pill ${item.state}`;$("detail-meta").textContent=`${item.image} · ${item.docker_id.slice(0,12)} · 最后出现 ${fmtTime(item.last_seen_at_ms)}`;
  $("detail-latest").innerHTML=[card("CPU",fmtPct(item.cpu_percent),"当前采样"),card("内存",fmtBytes(item.memory_working_set_bytes),fmtPct(item.memory_percent)),card("网络累计",`${fmtBytes(item.network_rx_bytes)} / ${fmtBytes(item.network_tx_bytes)}`,"接收 / 发送"),card("进程数",item.pids??"—","PID")].join("");
  drawChart($("cpu-chart"),points,[{key:"cpu_percent",color:"#52e5b5",name:"CPU",format:fmtPct}]);
  drawChart($("memory-chart"),points,[{key:"memory_working_set_bytes",color:"#6aa9ff",name:"工作集",format:fmtBytes},{key:"memory_limit_bytes",color:"#44536d",name:"限制",format:fmtBytes}]);
  drawChart($("network-chart"),points,[{key:"network_rx_bytes_per_second",color:"#52e5b5",name:"接收",format:fmtRate},{key:"network_tx_bytes_per_second",color:"#6aa9ff",name:"发送",format:fmtRate}]);
  drawChart($("block-chart"),points,[{key:"block_read_bytes_per_second",color:"#ffbe63",name:"读取",format:fmtRate},{key:"block_write_bytes_per_second",color:"#d781ff",name:"写入",format:fmtRate}]);
  renderEvents(eventResponse.events);$("analysis-link").href=`/api/v1/analysis/context?container_ids=${item.docker_id}&from=${response.from_ms}&to=${response.to_ms}`;
}
const card=(label,value,small)=>`<article class="stat-card"><span>${esc(label)}</span><strong>${esc(value)}</strong><small>${esc(small)}</small></article>`;
function renderEvents(events){$("event-list").innerHTML=events.length?events.slice().reverse().map(event=>`<li><time>${fmtTime(event.occurred_at_ms)}</time><span class="event-action">${esc(event.action)}</span><span>${event.exit_code==null?"Docker 容器事件":`退出码 ${event.exit_code}`}${event.oom_killed?" · OOM":""}</span></li>`).join(""):`<li class="empty">所选时间范围内暂无事件</li>`}

function drawChart(canvas,points,series){
  const rect=canvas.getBoundingClientRect(),dpr=Math.min(devicePixelRatio||1,2),width=Math.max(300,rect.width),height=rect.height||260;canvas.width=width*dpr;canvas.height=height*dpr;const ctx=canvas.getContext("2d");ctx.scale(dpr,dpr);ctx.clearRect(0,0,width,height);
  const pad={left:12,right:12,top:22,bottom:25},plotW=width-pad.left-pad.right,plotH=height-pad.top-pad.bottom;const values=series.flatMap(line=>points.map(point=>point[line.key]).filter(value=>value!=null&&Number.isFinite(value)));const max=Math.max(...values,1)*1.08;
  ctx.strokeStyle="#252f42";ctx.lineWidth=1;ctx.fillStyle="#718096";ctx.font="11px system-ui";ctx.textAlign="right";
  for(let i=0;i<=4;i++){const y=pad.top+plotH*i/4;ctx.beginPath();ctx.moveTo(pad.left,y);ctx.lineTo(width-pad.right,y);ctx.stroke();ctx.fillText(series[0].format(max*(4-i)/4),width-pad.right,y-4)}
  if(points.length<2){ctx.fillStyle="#718096";ctx.textAlign="center";ctx.fillText("等待更多采样数据",width/2,height/2);return}
  series.forEach((line,index)=>{ctx.strokeStyle=line.color;ctx.lineWidth=2;ctx.beginPath();let started=false;points.forEach((point,i)=>{const value=point[line.key];if(value==null||!Number.isFinite(value)){started=false;return}const x=pad.left+plotW*i/(points.length-1),y=pad.top+plotH*(1-value/max);if(!started){ctx.moveTo(x,y);started=true}else ctx.lineTo(x,y)});ctx.stroke();ctx.fillStyle=line.color;ctx.textAlign="left";ctx.fillRect(pad.left+index*110,pad.top-14,8,3);ctx.fillText(line.name,pad.left+12+index*110,pad.top-9)});
  ctx.fillStyle="#718096";ctx.textAlign="left";ctx.fillText(new Date(points[0].sampled_at_ms).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit"}),pad.left,height-5);ctx.textAlign="right";ctx.fillText(new Date(points.at(-1).sampled_at_ms).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit"}),width-pad.right,height-5)
}

$("refresh-button").addEventListener("click",refreshDashboard);$("search-input").addEventListener("input",renderRows);$("back-button").addEventListener("click",()=>navigate("/"));window.addEventListener("popstate",route);document.querySelectorAll("[data-nav]").forEach(link=>link.addEventListener("click",event=>{event.preventDefault();navigate(link.pathname)}));
$("range-buttons").addEventListener("click",event=>{const button=event.target.closest("button[data-range]");if(!button)return;document.querySelectorAll("#range-buttons button").forEach(item=>item.classList.toggle("active",item===button));app.range=Number(button.dataset.range);refreshDetail()});
window.addEventListener("resize",()=>{if(app.detailId)refreshDetail()});route();
