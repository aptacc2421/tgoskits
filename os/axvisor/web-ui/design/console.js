/* AxVisor 控制台视觉原型 v2 · 交互脚本（原生 JS，无依赖，支持中英切换） */

const DICT = {
  omni: { zh: '搜索客户机、配置、操作…', en: 'Search guests, configs, actions…' },
  evtConnected: { zh: '事件通道已连接', en: 'Event channel connected' },
  manifestChip: { zh: 'manifest proto 1 · 4 面板', en: 'manifest proto 1 · 4 panels' },
  navCapabilities: { zh: '能力 · 来自 manifest', en: 'Capabilities · from manifest' },
  navGuests: { zh: '客户机', en: 'Guests' },
  navHost: { zh: '宿主机', en: 'Host' },
  navConsole: { zh: '网页控制台', en: 'Console' },
  navFiles: { zh: '文件传输', en: 'Files' },
  navShell: { zh: 'Shell', en: 'Shell' },
  navLiveGuests: { zh: '客户机 · 实时', en: 'Guests · live' },
  sideFeatures: { zh: '构建特性 · http-axum, fs, browser-console', en: 'Features · http-axum, fs, browser-console' },
  guestsTitle: { zh: '客户机', en: 'Guests' },
  guestsDesc: { zh: '注册表来自', en: 'Registry from' },
  guestsDesc2: { zh: '；启动、暂停等动作返回后继续轮询，直到计数器或状态证明它真的生效。', en: '; start/pause keep polling after the reply until counters or status prove it landed.' },
  refresh: { zh: '刷新', en: 'Refresh' },
  pasteConfig: { zh: '粘贴配置', en: 'Paste config' },
  newGuest: { zh: '新建客户机', en: 'New guest' },
  kpiRunning: { zh: '运行中 / 已注册', en: 'Running / registered' },
  kpiRunningFoot: { zh: '近 5 分钟 +1', en: '+1 in last 5 min' },
  kpiVcpu: { zh: 'vCPU 已分配', en: 'vCPUs allocated' },
  kpiVcpuFoot: { zh: '{cpus} / {pcpus} 物理核 · {pin} 已绑核', en: '{cpus} / {pcpus} pCPUs · {pin} pinned' },
  kpiMem: { zh: '客户机内存', en: 'Guest memory' },
  kpiMemFoot: { zh: '不含宿主自身开销', en: 'Excludes host own overhead' },
  kpiEntry: { zh: 'guest 进入次数', en: 'guest entries' },
  kpiEntryFoot: { zh: 'entry 计数持续增长', en: 'entry counter still advancing' },
  registered: { zh: '已注册客户机', en: 'Registered guests' },
  all: { zh: '全部', en: 'All' },
  runningOnly: { zh: '运行中', en: 'Running' },
  stoppedOnly: { zh: '已停止', en: 'Stopped' },
  poolTitle: { zh: '候选配置', en: 'Config candidates' },
  poolNote: { zh: '条目只是候选，start 时才真正创建', en: 'Entries are candidates only; they become VMs on start' },
  browse: { zh: '浏览目录…', en: 'Browse…' },
  hostTitle: { zh: '宿主机', en: 'Host' },
  detail: { zh: '详情', en: 'Details' },
  fArch: { zh: '架构 / 平台', en: 'Arch / platform' },
  fPcpu: { zh: '物理核', en: 'Physical CPUs' },
  fControl: { zh: '控制面', en: 'Control plane' },
  fPool: { zh: '配置池', en: 'Guest pool' },
  fUptime: { zh: '运行时长', en: 'Uptime' },
  activityTitle: { zh: '最近动作', en: 'Recent activity' },
  activityNote: { zh: '事件通道实时推送', en: 'Pushed over the event channel' },
  hostDesc: { zh: '静态字段来自编译期常量与 manifest 的 host 段；运行时长与占用为本次请求的读数。', en: 'Static fields come from compile-time constants and the manifest host section; uptime and usage are read per request.' },
  hostSystem: { zh: '系统', en: 'System' },
  fProduct: { zh: '产品', en: 'Product' },
  fVersion: { zh: '版本', en: 'Version' },
  fSmp: { zh: 'SMP', en: 'SMP' },
  fBuild: { zh: '构建目标', en: 'Target' },
  hostAlloc: { zh: '资源分配', en: 'Allocation' },
  hostAllocNote: { zh: '合计自 VM 注册表', en: 'Summed from the VM registry' },
  hostFeatures: { zh: '构建特性', en: 'Build features' },
  hostRuntime: { zh: '运行状况', en: 'Runtime' },
  fWeb: { zh: 'Web 控制台地址', en: 'Web console URL' },
  fBind: { zh: '监听', en: 'Listen' },
  fManifest: { zh: '清单', en: 'Manifest' },
  fEvents: { zh: '事件通道', en: 'Event channel' },
  hostAffinity: { zh: 'vCPU 亲和矩阵', en: 'vCPU affinity matrix' },
  hostAffinityNote: { zh: '来自 phys_cpu_set', en: 'from phys_cpu_set' },
  legendPinned: { zh: '已绑定物理核', en: 'pinned to a pCPU' },
  legendFree: { zh: '空闲', en: 'free' },
  exclusive: { zh: '独占通道', en: 'exclusive lane' },
  clear: { zh: '清空', en: 'Clear' },
  disconnect: { zh: '断开', en: 'Disconnect' },
  idleLane: { zh: '空闲', en: 'idle' },
  busyLane: { zh: '占用', en: 'in use' },
  filesTitle: { zh: '文件传输', en: 'File transfer' },
  filesDesc: { zh: '先把字节传到宿主暂存区，再由', en: 'Bytes land in the host staging area first, then' },
  filesDesc2: { zh: '落到目标目录——配置里引用的路径必须是已经落地的文件。', en: 'moves them to the target: a config may only reference a file that is already placed.' },
  dropTitle: { zh: '拖入镜像、内核或客户机配置', en: 'Drop an image, kernel or guest config' },
  dropDesc: { zh: '支持断点续传：重开同名会话会从已写入的字节继续', en: 'Resumable: reopening the same session continues from bytes on disk' },
  transfers: { zh: '传输', en: 'Transfers' },
  shellDesc: { zh: '宿主机 Shell 通道，与控制台同源的 WebSocket 终端。', en: 'Host shell channel — same WebSocket terminal stack as the console.' },
  sbEvents: { zh: '事件通道 /ws/events 已连接', en: 'Event channel /ws/events connected' },
  dEntryLabel: { zh: 'guest_entry_count · 真正进入 guest 的次数', en: 'guest_entry_count · real guest re-entries' },
  dParkLabel: { zh: 'guest_park_count · 真正 park 的次数', en: 'guest_park_count · real park events' },
  dVcpuLabel: { zh: 'vCPU 状态与亲和（每个格子一个 vCPU）', en: 'vCPU state and affinity (one cell per vCPU)' },
  dVcpuLegend: { zh: '蓝=已启动，青=已绑定物理核，灰=未启动', en: 'blue=up, cyan=pinned, grey=down' },
  thStatus: { zh: '状态', en: 'Status' },
  thMem: { zh: '内存', en: 'Memory' },
  detailApi: { zh: '详情接口', en: 'Detail route' },
  actPause: { zh: '暂停', en: 'Pause' },
  actStop: { zh: '停止', en: 'Stop' },
  actConsole: { zh: '打开控制台', en: 'Open console' },
  actDestroy: { zh: '销毁', en: 'Destroy' },
  unitCore: { zh: '核', en: 'cores' },
  pinned: { zh: '已绑核', en: 'pinned' },
  unpinned: { zh: '未绑核', en: 'unpinned' },
  start: { zh: '启动', en: 'Start' },
  allocVcpu: { zh: 'vCPU 分配', en: 'vCPU allocation' },
  allocOvercommit: { zh: '超配比', en: 'overcommit' },
  allocMem: { zh: '客户机内存合计', en: 'Total guest memory' },
  allocMemNote: { zh: '宿主总量不由控制面导出', en: 'host total is not exported by the control plane' },
  allocGuests: { zh: '客户机', en: 'Guests' },
  allocLanes: { zh: '控制台通道', en: 'Console lanes' },
  hostOwn: { zh: '宿主机自用', en: 'host own' },
  st_running: { zh: '运行中', en: 'running' },
  st_ready: { zh: '就绪', en: 'ready' },
  st_paused: { zh: '已暂停', en: 'paused' },
  st_stopped: { zh: '已停止', en: 'stopped' },
  st_failed: { zh: '失败', en: 'failed' },
  ph_placed: { zh: '已落地', en: 'placed' },
  ph_sending: { zh: '传输中', en: 'sending' },
  ph_needsname: { zh: '待命名', en: 'awaiting name' },
  cmd1: { zh: '启动 VM[2] guest-linux-x86_64', en: 'Start VM[2] guest-linux-x86_64' },
  cmd2: { zh: '打开 VM[1] 的网页控制台', en: 'Open VM[1] browser console' },
  cmd3: { zh: '新建客户机（字段来自 schema）', en: 'Create a guest (fields from schema)' },
  cmd4: { zh: '浏览 /guest 目录', en: 'Browse /guest' },
  cmd5: { zh: '查看宿主机信息', en: 'Show host information' },
  cmd6: { zh: '重新读取能力清单', en: 'Re-read the capability manifest' },
};

const STATUS = {
  running: { cls: 's-running', pulse: true, key: 'st_running' },
  ready: { cls: 's-ready', pulse: false, key: 'st_ready' },
  paused: { cls: 's-paused', pulse: false, key: 'st_paused' },
  stopped: { cls: 's-stopped', pulse: false, key: 'st_stopped' },
  failed: { cls: 's-failed', pulse: true, key: 'st_failed' },
};

const PCPUS = 8;

const VMS = [
  { id: 1, name: 'guest-linux-aarch64', status: 'running', cpu: 4, mem: 1024, entry: 18204, park: 7931, pins: [0, 1, 2, 3] },
  { id: 2, name: 'guest-linux-x86_64', status: 'ready', cpu: 2, mem: 512, entry: 0, park: 0, pins: [] },
  { id: 3, name: 'guest-nimbos-aarch64', status: 'running', cpu: 2, mem: 1024, entry: 9210, park: 4102, pins: [4, 5] },
  { id: 4, name: 'guest-arceos-riscv64', status: 'paused', cpu: 2, mem: 256, entry: 1180, park: 640, pins: [] },
  { id: 5, name: 'guest-linux-riscv64', status: 'failed', cpu: 2, mem: 512, entry: 0, park: 0, pins: [] },
];

const POOL = [
  { id: 2, name: 'guest-linux-x86_64.toml', path: '/guest/x86_64/linux.toml' },
  { id: 6, name: 'guest-arceos-aarch64.toml', path: '/guest/aarch64/arceos.toml' },
  { id: 7, name: 'guest-nimbos-riscv64.toml', path: '/guest/riscv64/nimbos.toml' },
];

const LANES = [
  { id: 1, attached: true, active: true },
  { id: 3, attached: false, active: false },
  { id: 5, attached: false, active: false },
];

const TRANSFERS = [
  { name: 'Image-aarch64', dir: '/guest/aarch64', written: 24_117_248, total: 24_117_248, phase: 'placed' },
  { name: 'rootfs.ext4', dir: '/guest/aarch64', written: 41_943_040, total: 67_108_864, phase: 'sending' },
  { name: 'guest-new.toml', dir: '/guest', written: 1024, total: 1024, phase: 'needs-name' },
];

const ACTIVITY = [
  { t: { zh: '刚刚', en: 'now' }, text: { zh: 'VM[1] 启动完成：entry 计数 +18,204', en: 'VM[1] started: entry counter +18,204' }, tone: 'ok' },
  { t: { zh: '12s', en: '12s' }, text: { zh: 'VM[4] 已暂停，vCPU 全部 park', en: 'VM[4] paused, all vCPUs parked' }, tone: 'warn' },
  { t: { zh: '1m', en: '1m' }, text: { zh: 'rootfs.ext4 传输中 · 62%', en: 'rootfs.ext4 transferring · 62%' }, tone: '' },
  { t: { zh: '3m', en: '3m' }, text: { zh: 'VM[5] 创建失败：镜像路径不存在', en: 'VM[5] create failed: image path missing' }, tone: 'err' },
  { t: { zh: '6m', en: '6m' }, text: { zh: '重新读取能力清单 · proto 1', en: 'Manifest re-read · proto 1' }, tone: '' },
];

const BOOT_LOG = [
  ['[    0.000000]', 'AxVisor 0.8.2 · aarch64 · 4 vCPU · 1024 MiB', 'hi'],
  ['[    0.004312]', 'loading guest image /guest/aarch64/linux/Image', 'dim'],
  ['[    0.088141]', 'VM[1] vcpu0 -> EL1  entry=1', 'ok'],
  ['[    0.091507]', 'VM[1] vcpu1 -> EL1  entry=1', 'ok'],
  ['[    0.094233]', 'VM[1] vcpu2 -> EL1  entry=1', 'ok'],
  ['[    0.097884]', 'VM[1] vcpu3 -> EL1  entry=1', 'ok'],
  ['[    0.142110]', 'Linux version 6.9.0 (aarch64) gcc 13.2.0', ''],
  ['[    0.203551]', 'Booting Linux on physical CPU 0x0000000000 [0x411fd070]', 'dim'],
  ['[    0.310442]', 'Memory: 1014528K/1048576K available', 'dim'],
  ['[    0.418907]', 'virtio_blk virtio0: [vda] 2097152 512-byte logical blocks', 'ok'],
  ['[    0.512330]', 'VFS: Mounted root (ext4 filesystem) on device 254:0', 'ok'],
  ['[    0.604118]', 'warning: console lane is exclusive, second browser refused', 'warn'],
  ['[    0.710045]', 'systemd[1]: Detected virtualization axvisor.', 'dim'],
  ['[    0.883214]', 'Run /sbin/init as init process', ''],
  ['[    1.204551]', 'login: ', 'hi'],
];

/* ---------------- i18n ---------------- */
let lang = 'zh';
const pick = (v) => (v && typeof v === 'object' && !Array.isArray(v) ? v[lang] : v);
function t(key, vars) {
  const entry = DICT[key];
  let s = entry ? entry[lang] : key;
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, v);
  return s;
}
function applyStatic() {
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
  for (const node of document.querySelectorAll('[data-i18n]')) {
    const key = node.dataset.i18n;
    if (key in DICT) node.textContent = DICT[key][lang];
  }
  for (const node of document.querySelectorAll('[data-both]')) {
    const [zh, en] = node.dataset.both.split('|');
    node.textContent = lang === 'zh' ? zh : en;
  }
}

/* ---------------- 工具 ---------------- */
const $ = (sel) => document.querySelector(sel);
const el = (tag, cls, html) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (html !== undefined) n.innerHTML = html;
  return n;
};
const fmtBytes = (b) => (b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MiB` : `${Math.round(b / 1024)} KiB`);
const fmtNum = (n) => n.toLocaleString('en-US');
const sum = (fn) => VMS.reduce((s, v) => s + fn(v), 0);
const pinnedCpus = () => VMS.reduce((s, v) => s + v.pins.length, 0);

/* ---------------- 渲染：KPI 与列表 ---------------- */
function renderKpis() {
  const running = VMS.filter((v) => v.status === 'running').length;
  const cpus = sum((v) => v.cpu);
  const mem = sum((v) => v.mem);
  const entry = sum((v) => v.entry);
  const values = [
    `${running}<small>/ ${VMS.length}</small>`,
    `${cpus}<small> ${t('unitCore')}</small>`,
    `${(mem / 1024).toFixed(1)}<small> GiB</small>`,
    `${(entry / 1000).toFixed(1)}<small>k</small>`,
  ];
  document.querySelectorAll('.kpi-value').forEach((n, i) => (n.innerHTML = values[i]));
  const foots = document.querySelectorAll('.kpi-foot > span');
  foots[0].textContent = t('kpiRunningFoot');
  foots[1].textContent = t('kpiVcpuFoot', { cpus, pcpus: PCPUS, pin: pinnedCpus() });
  foots[2].textContent = t('kpiMemFoot');
  foots[3].textContent = t('kpiEntryFoot');
  $('#sbRes').textContent = `${VMS.length} ${lang === 'zh' ? '客户机' : 'guests'} · ${cpus} vCPU · ${(mem / 1024).toFixed(1)} GiB`;
}

function renderVms() {
  const list = $('#vmList');
  list.innerHTML = '';
  for (const vm of VMS) {
    const s = STATUS[vm.status];
    const row = el('div', 'vm-row');
    row.dataset.id = vm.id;
    row.innerHTML = `
      <div class="vm-id">
        <span class="dot ${s.cls} ${s.pulse ? 'pulse' : ''}"></span>
        <div style="min-width: 0">
          <div class="vm-name">${vm.name}</div>
          <div class="vm-sub">VM[${vm.id}] · ${t(s.key)}</div>
        </div>
      </div>
      <div class="res-col">
        <div class="res-line">
          <svg width="12" height="12" style="color: var(--text-mute)"><use href="#i-memory"/></svg>
          <span>${t('thMem')}</span>
          <span class="val">${vm.mem} MiB</span>
        </div>
        <div class="meter"><i style="width: ${Math.round((vm.mem / 1024) * 100)}%"></i></div>
      </div>
      <div class="res-col">
        <div class="res-line">
          <svg width="12" height="12" style="color: var(--text-mute)"><use href="#i-cpu"/></svg>
          <span>${vm.cpu} vCPU</span>
          <span class="val">${vm.pins.length ? t('pinned') : t('unpinned')}</span>
        </div>
        <div class="vcpu-cells">${Array.from({ length: vm.cpu }, (_, i) =>
          `<span class="vcpu-cell ${vm.pins.includes(i) ? 'pin' : vm.status === 'running' || vm.status === 'paused' ? 'on' : ''}"></span>`,
        ).join('')}</div>
      </div>
      <div class="row-actions">
        ${vm.status === 'running'
          ? `<button class="icon-btn" title="Pause" data-toast="VM[${vm.id}] 暂停请求已提交|VM[${vm.id}] pause requested"><svg width="15" height="15"><use href="#i-pause"/></svg></button>
             <button class="icon-btn" title="Stop" data-toast="VM[${vm.id}] 停止请求已提交|VM[${vm.id}] stop requested"><svg width="15" height="15"><use href="#i-stop"/></svg></button>`
          : `<button class="icon-btn" title="Start" data-toast="VM[${vm.id}] 启动中，等待 entry 计数增长…|VM[${vm.id}] starting, waiting for entry counter"><svg width="15" height="15"><use href="#i-play"/></svg></button>`}
        <button class="icon-btn" title="More"><svg width="15" height="15"><use href="#i-more"/></svg></button>
      </div>`;
    row.addEventListener('click', (e) => {
      if (e.target.closest('.icon-btn')) return;
      openDrawer(vm);
    });
    list.appendChild(row);
  }
}

function renderNavVms() {
  const box = $('#navVms');
  box.innerHTML = '';
  for (const vm of VMS) {
    const s = STATUS[vm.status];
    const b = el('button', 'vm-mini');
    b.innerHTML = `
      <span class="dot ${s.cls} ${s.pulse ? 'pulse' : ''}" style="width: 6px; height: 6px"></span>
      <span class="mono" style="font-size: 11.5px">VM[${vm.id}]</span>
      <span style="margin-left: auto; font-size: 10.5px; color: var(--text-mute)">${t(s.key)}</span>`;
    b.addEventListener('click', () => openDrawer(vm));
    box.appendChild(b);
  }
}

function renderPool() {
  const box = $('#poolList');
  box.innerHTML = '';
  for (const p of POOL) {
    const row = el('div', 'pool-row');
    row.innerHTML = `
      <span class="tag">VM[${p.id}]</span>
      <span>${p.name}</span>
      <span class="path grow">${p.path}</span>
      <button class="btn sm" data-toast="从 ${p.path} 创建并启动客户机|Creating from ${p.path}">${t('start')}</button>`;
    box.appendChild(row);
  }
  const issue = el('div', 'pool-row');
  issue.innerHTML = `
    <span class="tag warn">duplicate-id</span>
    <span style="color: var(--text-dim)">/guest/x86_64/linux-copy.toml</span>
    <span class="path grow">${lang === 'zh' ? `与 VM[2] 的 id 冲突，已跳过` : `id conflicts with VM[2], skipped`}</span>`;
  box.appendChild(issue);
}

function renderActivity() {
  const box = $('#activity');
  box.innerHTML = '';
  for (const a of ACTIVITY) {
    const row = el('div', 'pool-row');
    row.style.padding = '8px 0';
    row.innerHTML = `
      <span class="mono" style="font-size: 11px; color: var(--text-mute); width: 34px">${pick(a.t)}</span>
      <span style="flex: 1; color: ${a.tone === 'err' ? 'var(--danger)' : a.tone === 'warn' ? 'var(--warn)' : a.tone === 'ok' ? 'var(--ok)' : 'var(--text-dim)'}">${pick(a.text)}</span>`;
    box.appendChild(row);
  }
}

/* ---------------- 渲染：宿主机 ---------------- */
function renderAlloc() {
  const cpus = sum((v) => v.cpu);
  const mem = sum((v) => v.mem);
  const running = VMS.filter((v) => v.status === 'running').length;
  const lanes = LANES.filter((l) => l.attached).length;
  const rows = [
    {
      label: t('allocVcpu'),
      note: `${cpus} / ${PCPUS} pCPU · ${t('allocOvercommit')} ${(cpus / PCPUS).toFixed(2)}×`,
      pct: Math.min(100, Math.round((cpus / PCPUS) * 100)),
    },
    {
      label: t('allocMem'),
      note: `${mem} MiB · ${lang === 'zh' ? '宿主总量不由控制面导出' : 'host total not exported'}`,
      pct: null,
    },
    { label: t('allocGuests'), note: `${running} ${lang === 'zh' ? '运行中' : 'running'} / ${VMS.length}`, pct: Math.round((running / VMS.length) * 100) },
    { label: t('allocLanes'), note: `${lanes} / ${LANES.length}`, pct: Math.round((lanes / LANES.length) * 100) },
  ];
  const box = $('#allocBox');
  box.innerHTML = '';
  for (const r of rows) {
    const div = el('div', 'alloc-row');
    div.innerHTML = `
      <div class="top"><span>${r.label}</span><span class="n">${r.note}</span></div>
      ${r.pct === null ? '' : `<div class="meter"><i style="width: ${r.pct}%"></i></div>`}`;
    box.appendChild(div);
  }
}

function renderMatrix() {
  const head = Array.from({ length: PCPUS }, (_, i) => `<span>P${i}</span>`).join('');
  const lines = VMS.map((vm) => {
    const cells = Array.from({ length: PCPUS }, (_, i) =>
      `<span class="mcell ${vm.pins.includes(i) ? 'pin' : ''}"></span>`,
    ).join('');
    return `<div class="matrix-row"><span class="who mono">VM[${vm.id}]</span>${cells}</div>`;
  }).join('');
  const hostRow = `<div class="matrix-row"><span class="who">${t('hostOwn')}</span>${Array.from(
    { length: PCPUS },
    (_, i) => `<span class="mcell ${VMS.some((v) => v.pins.includes(i)) ? '' : 'self'}"></span>`,
  ).join('')}</div>`;
  const box = $('#matrix');
  box.style.setProperty('--pcpus', String(PCPUS));
  box.innerHTML = `
    <div class="matrix-head"><span></span>${head}</div>
    <div class="matrix">${lines}${hostRow}</div>`;
}

/* ---------------- 控制台 / Shell / 文件 ---------------- */
function renderLanes() {
  const box = $('#laneList');
  box.innerHTML = '';
  for (const l of LANES) {
    const b = el('button', `lane ${l.active ? 'active' : ''}`);
    b.innerHTML = `
      <span class="dot ${l.attached ? 's-running' : 's-stopped'}" style="width: 6px; height: 6px"></span>
      <span class="mono">VM[${l.id}]</span>
      <span class="lane-tag">${l.attached ? t('busyLane') : t('idleLane')}</span>`;
    b.addEventListener('click', () => {
      document.querySelectorAll('.lane').forEach((n) => n.classList.remove('active'));
      b.classList.add('active');
      printLog($('#term'), BOOT_LOG);
    });
    box.appendChild(b);
  }
}

function printLog(node, lines) {
  node.innerHTML = '';
  let i = 0;
  const timer = setInterval(() => {
    if (i >= lines.length) {
      clearInterval(timer);
      node.appendChild(el('span', 'cursor'));
      node.scrollTop = node.scrollHeight;
      return;
    }
    const [ts, text, tone] = lines[i++];
    node.appendChild(el('div', '', `<span class="ts">${ts}</span> <span class="${tone}">${text}</span>`));
    node.scrollTop = node.scrollHeight;
  }, 90);
}

function renderShell() {
  $('#shellTerm').innerHTML = `<div><span class="dim">root@axvisor</span>:<span class="hi">/guest</span># ls -la</div>
<div class="dim">drwxr-xr-x 4 root root 4096 aarch64</div>
<div class="dim">drwxr-xr-x 3 root root 4096 x86_64</div>
<div class="dim">-rw-r--r-- 1 root root 1024 guest-new.toml</div>
<div><span class="dim">root@axvisor</span>:<span class="hi">/guest</span># <span class="cursor"></span></div>`;
}

function renderTransfers() {
  const box = $('#transferList');
  box.innerHTML = '';
  for (const tr of TRANSFERS) {
    const pct = Math.round((tr.written / tr.total) * 100);
    const row = el('div', 'tf-row');
    row.innerHTML = `
      <div class="tf-name">
        <span class="dot ${tr.phase === 'placed' ? 's-running' : tr.phase === 'sending' ? 's-ready' : 's-paused'}" style="width: 6px; height: 6px"></span>
        <span>${tr.name}</span>
        <span class="path mono" style="font-size: 11px; color: var(--text-mute)">${tr.dir}</span>
      </div>
      <div class="tf-bar"><i style="width: ${pct}%"></i></div>
      <span class="mono" style="font-size: 11.5px; color: var(--text-dim)">${fmtBytes(tr.written)} / ${fmtBytes(tr.total)}</span>
      <button class="icon-btn" title="${t('ph_' + tr.phase.replace('-', ''))}" data-toast="${tr.name} · ${t('ph_' + tr.phase.replace('-', ''))}">
        <svg width="14" height="14"><use href="#i-more"/></svg>
      </button>`;
    box.appendChild(row);
  }
}

function renderCmdk() {
  const items = [
    ['cmd1', '#i-play', '↵'],
    ['cmd2', '#i-terminal', 'console'],
    ['cmd3', '#i-plus', 'create'],
    ['cmd4', '#i-folder', 'files'],
    ['cmd5', '#i-host', 'host'],
    ['cmd6', '#i-refresh', 'manifest'],
  ];
  $('#cmdkList').innerHTML = items
    .map(
      ([key, icon, go], i) =>
        `<button class="cmdk-item ${i === 0 ? 'sel' : ''}"><svg width="15" height="15"><use href="${icon}"/></svg>${t(key)}<span class="go">${go}</span></button>`,
    )
    .join('');
  $('#cmdkText').textContent = lang === 'zh' ? '输入命令或客户机名称…' : 'Type a command or guest name…';
}

/* ---------------- 抽屉 ---------------- */
function openDrawer(vm) {
  const s = STATUS[vm.status];
  $('#dTitle').textContent = `VM[${vm.id}]`;
  $('#dSub').textContent = `${vm.name} · ${vm.cpu} vCPU · ${vm.mem} MiB`;
  $('#dStatus').textContent = vm.status;
  $('#dState').textContent = t(s.key);
  $('#dId').textContent = vm.id;
  $('#dCpu').textContent = vm.cpu;
  $('#dMem').textContent = `${vm.mem} MiB`;
  $('#dEntry').textContent = fmtNum(vm.entry);
  $('#dPark').textContent = fmtNum(vm.park);
  $('#dUrl').textContent = `GET /api/vms/${vm.id}`;
  $('#dDot').className = `dot ${s.cls} ${s.pulse ? 'pulse' : ''}`;
  $('#dVcpu').innerHTML = Array.from({ length: vm.cpu }, (_, i) =>
    `<span class="vcpu-cell ${vm.pins.includes(i) ? 'pin' : vm.status === 'running' || vm.status === 'paused' ? 'on' : ''}" style="width: 16px; height: 22px"></span>`,
  ).join('');
  document.querySelectorAll('.vm-row').forEach((r) => r.classList.toggle('selected', Number(r.dataset.id) === vm.id));
  $('#drawer').classList.add('show');
  $('#scrim').classList.add('show');
}

function closeDrawer() {
  $('#drawer').classList.remove('show');
  $('#scrim').classList.remove('show');
  document.querySelectorAll('.vm-row').forEach((r) => r.classList.remove('selected'));
}

/* ---------------- 视图 / 主题 / 语言 ---------------- */
function switchView(name) {
  document.querySelectorAll('.nav-item').forEach((n) => n.classList.toggle('active', n.dataset.view === name));
  document.querySelectorAll('.view').forEach((v) => v.classList.toggle('active', v.id === `view-${name}`));
  if (name === 'console' && $('#term').childElementCount === 0) printLog($('#term'), BOOT_LOG);
}

function setTheme(next) {
  document.documentElement.dataset.theme = next;
  $('#themeToggle').innerHTML =
    next === 'dark'
      ? '<svg width="16" height="16"><use href="#i-moon"/></svg>'
      : '<svg width="16" height="16"><use href="#i-sun"/></svg>';
}

function toast(raw) {
  const parts = raw.split('|');
  const text = parts.length > 1 ? (lang === 'zh' ? parts[0] : parts[1]) : raw;
  const node = el('div', 'toast', `<span class="dot s-ready" style="width: 6px; height: 6px"></span>${text}`);
  $('#toasts').appendChild(node);
  setTimeout(() => node.remove(), 2600);
}

function setLang(next) {
  lang = next;
  document.querySelectorAll('#langSwitch button').forEach((b) => b.classList.toggle('on', b.dataset.lang === next));
  renderAll();
}

function renderAll() {
  applyStatic();
  renderKpis();
  renderVms();
  renderNavVms();
  renderPool();
  renderActivity();
  renderAlloc();
  renderMatrix();
  renderLanes();
  renderTransfers();
  renderCmdk();
}

/* ---------------- 时间 ---------------- */
let uptimeSec = 2 * 86400 + 4 * 3600 + 12 * 60 + 33;
function fmtUptime(s) {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n) => String(n).padStart(2, '0');
  return `${d}d ${pad(h)}:${pad(m)}:${pad(sec)}`;
}
function tickTime() {
  uptimeSec += 1;
  $('#uptimeMini').textContent = fmtUptime(uptimeSec);
  $('#uptimeFull').textContent = fmtUptime(uptimeSec);
  $('#clock').textContent = new Date().toLocaleTimeString(lang === 'zh' ? 'zh-CN' : 'en-GB', { hour12: false });
}

/* ---------------- 绑定 ---------------- */
document.querySelectorAll('.nav-item').forEach((n) => n.addEventListener('click', () => switchView(n.dataset.view)));
document.querySelectorAll('[data-goto]').forEach((n) => n.addEventListener('click', () => switchView(n.dataset.goto)));
document.querySelectorAll('#langSwitch button').forEach((b) => b.addEventListener('click', () => setLang(b.dataset.lang)));
$('#themeToggle').addEventListener('click', () => setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'));
$('#openCmdk').addEventListener('click', () => $('#cmdk').classList.add('show'));
$('#cmdk').addEventListener('click', () => $('#cmdk').classList.remove('show'));
$('#closeDrawer').addEventListener('click', closeDrawer);
$('#scrim').addEventListener('click', closeDrawer);
document.addEventListener('click', (e) => {
  const btn = e.target.closest('[data-toast]');
  if (btn) toast(btn.dataset.toast);
});
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    $('#cmdk').classList.toggle('show');
  }
  if (e.key === 'Escape') {
    $('#cmdk').classList.remove('show');
    closeDrawer();
  }
});

/* URL hash 便于直接分享某一屏：#view=host&theme=light&lang=en */
function applyHash() {
  const p = new URLSearchParams(location.hash.replace(/^#/, ''));
  if (p.get('theme')) setTheme(p.get('theme'));
  if (p.get('view')) switchView(p.get('view'));
  const next = p.get('lang');
  if (next && next !== lang) setLang(next);
}

renderShell();
setTheme('dark');
setLang('zh');
applyHash();
tickTime();
setInterval(tickTime, 1000);
setInterval(() => {
  const tr = TRANSFERS.find((x) => x.phase === 'sending');
  if (!tr) return;
  tr.written = tr.written + 1_800_000 >= tr.total ? 0 : tr.written + 1_800_000;
  renderTransfers();
}, 1200);
