/* AxVisor console demo — data shape follows the real control-plane contract. */

const PCPUS = 8;
const BOOT = Date.now() - 3 * 3600 * 1000 - 42 * 60 * 1000;

const HOST = {
  version: '0.8.2',
  arch: 'x86_64',
  platform: 'x86_64-qemu-q35',
  smp: 8,
  cpus: 8,
  web: 'http://10.0.2.15:8080',
  listen: '0.0.0.0:8080',
  features: ['fs', 'http-axum', 'browser-console', 'web-ui'],
  pool: ['/guest', 'AXVISOR_VM_DIRS'],
  events: '/ws/events',
};

const VMS = [
  { id: 'vm-1', name: 'arceos-shell', status: 'Running', vcpus: 4, mem: 1.0, cpus: [0, 1, 2, 3], entry: 12, park: 3, image: 'arceos-x86_64.img', cmdline: 'LOG=info' },
  { id: 'vm-2', name: 'linux-guest', status: 'Running', vcpus: 4, mem: 2.0, cpus: [4, 5, 6, 7], entry: 8, park: 1, image: 'linux-x86_64.img', cmdline: 'console=ttyS0' },
  { id: 'vm-3', name: 'rtos-node', status: 'Paused', vcpus: 2, mem: 0.5, cpus: [0, 1], entry: 5, park: 5, image: 'rtos-x86_64.img', cmdline: '' },
  { id: 'vm-4', name: 'net-peer', status: 'Stopped', vcpus: 2, mem: 0.5, cpus: [2, 3], entry: 0, park: 0, image: 'net-x86_64.img', cmdline: '' },
  { id: 'vm-5', name: 'blk-bench', status: 'Failed', vcpus: 1, mem: 4.0, cpus: [7], entry: 2, park: 0, image: 'bench-x86_64.img', cmdline: '', issue: 'duplicate-id' },
];

const STATE_CN = { Running: '运行中', Paused: '已暂停', Stopped: '已停止', Failed: '失败' };

const VIEWS = [
  { id: 'vms', label: '客户机', icon: 'i-server', group: '能力' },
  { id: 'console', label: '网页控制台', icon: 'i-terminal', group: '能力' },
  { id: 'files', label: '文件传输', icon: 'i-file', group: '能力' },
  { id: 'shell', label: 'Shell', icon: 'i-shell', group: '能力' },
  { id: 'host', label: '宿主机', icon: 'i-cpu', group: '系统' },
];

const TRANSFERS = [
  { name: 'rootfs.ext4', total: 268435456, done: 268435456, state: '完成' },
  { name: 'app.bin', total: 12582912, done: 7340032, state: '传输中' },
  { name: 'config.toml', total: 4096, done: 0, state: '排队' },
];

const BOOTLOG = [
  'Booting AxVisor 0.8.2 (x86_64-qemu-q35)',
  '  phys memory regions: 3',
  '  percpu areas: 8 cpus',
  'Initializing guest vm-1 "arceos-shell"',
  '  vcpu: 4  pinned: 0-3',
  '  memory: 1.000 GiB',
  '  entry: 0x8000  dtb: 0x44000000',
  'VMM setup done, entering guest',
  '[    0.112] ArceOS starting...',
  '[    0.118] axhal: x86_64 initialized',
  '[    0.124] axalloc: bitmap allocator ready',
  '[    0.131] axtask: 4 tasks spawned',
  '[    0.140] axfs: mounting ramfs at /',
  '[    0.152] shell: arceos shell ready',
];

/* ---------- helpers ---------- */
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
const icon = (id, size = 15) =>
  `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><use href="#${id}" /></svg>`;
const memStr = (g) => (g >= 1 ? `${g.toFixed(1)} GiB` : `${Math.round(g * 1024)} MiB`);
const bytes = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`);

const state = {
  view: 'vms',
  theme: localStorage.getItem('axvisor.theme') || 'industrial',
  selected: null,
  cmdkOpen: false,
  cmdkIndex: 0,
  query: '',
};

/* ---------- nav ---------- */
function renderNav() {
  const groups = [...new Set(VIEWS.map((v) => v.group))];
  $('#nav').innerHTML = groups
    .map((g) => {
      const items = VIEWS.filter((v) => v.group === g)
        .map(
          (v) => `<button class="nav-item" data-view="${v.id}" aria-current="${state.view === v.id ? 'page' : 'false'}">
            ${icon(v.icon)}
            <span>${v.label}</span>
            ${v.id === 'vms' ? `<span class="nav-count">${VMS.length}</span>` : ''}
          </button>`
        )
        .join('');
      return `<div class="group"><div class="group-label">${g}</div>${items}</div>`;
    })
    .join('');
}

/* ---------- views ---------- */
function viewVms() {
  const running = VMS.filter((v) => v.status === 'Running').length;
  const vcpu = VMS.reduce((a, v) => a + v.vcpus, 0);
  const mem = VMS.reduce((a, v) => a + v.mem, 0);
  const entry = VMS.reduce((a, v) => a + v.entry, 0);
  const ratio = vcpu / PCPUS;

  const rows = VMS.map((v) => {
    const acts =
      v.status === 'Running'
        ? `${mini('i-pause', 'pause', '暂停')}${mini('i-stop', 'stop', '停止')}`
        : `${mini('i-play', 'start', '启动')}${mini('i-x', 'destroy', '关闭')}`;
    return `<div class="guest" data-status="${v.status}" data-open="${v.id}" tabindex="0" role="button" aria-label="${esc(v.name)} 详情">
      <span class="guest-bar"></span>
      <span class="guest-name"><b>${esc(v.name)}</b><span>${v.id} · ${esc(v.image)}</span></span>
      <span class="state" data-s="${v.status}"><i></i>${STATE_CN[v.status]}</span>
      <span class="alloc">
        <span class="alloc-num">${v.vcpus} <em>vCPU</em></span>
        <span class="bar"><i style="width:${Math.round((v.vcpus / PCPUS) * 100)}%"></i></span>
      </span>
      <span class="alloc">
        <span class="alloc-num">${memStr(v.mem)}</span>
        <span class="bar"><i style="width:${Math.round((v.mem / 8) * 100)}%"></i></span>
      </span>
      <span class="guest-actions" data-actions>${acts}</span>
    </div>`;
  }).join('');

  const pinned = new Set(VMS.filter((v) => v.status !== 'Stopped').flatMap((v) => v.cpus));
  const lanes = VMS.map(
    (v) => `<div class="slot-row" data-lane="${v.id}">
      <span class="who">${esc(v.name)}</span>
      <span class="lane">${Array.from({ length: PCPUS }, (_, i) => `<span class="cell ${v.cpus.includes(i) ? 'on' : ''}"></span>`).join('')}</span>
      <span class="slot-n">${v.cpus.length}/${PCPUS}</span>
    </div>`
  ).join('');
  const hostLane = `<div class="slot-row">
      <span class="who">宿主自用</span>
      <span class="lane">${Array.from({ length: PCPUS }, (_, i) => `<span class="cell ${pinned.has(i) ? '' : 'host'}"></span>`).join('')}</span>
      <span class="slot-n">${PCPUS - pinned.size}/${PCPUS}</span>
    </div>`;

  return `<div class="view view-enter">
    <div class="page-head">
      <div>
        <h1 class="page-title">客户机</h1>
        <p class="page-sub">配置池 /guest 扫描到 ${VMS.length} 个候选配置</p>
      </div>
      <div class="page-actions">
        <button class="btn">${icon('i-refresh', 14)} 重新扫描</button>
        <button class="btn btn-primary">${icon('i-plus', 14)} 启动客户机</button>
      </div>
    </div>

    <div class="band">
      <div class="band-lead">
        <span class="band-big">${VMS.length}</span>
        <span class="band-lead-label">台客户机<br />${running} 台运行中</span>
      </div>
      <div class="metrics">
        <div>
          <div class="metric-label">已分配 vCPU</div>
          <div class="metric-value">${vcpu} <span style="color:var(--text-3)">/ ${PCPUS} pCPU</span></div>
          <div class="metric-note">超配比 ${ratio.toFixed(1)}×</div>
        </div>
        <div class="ratio">
          <div class="metric-label">物理核覆盖</div>
          <div class="ratio-track"><span class="ratio-fill" style="width:${Math.min(100, ratio * 100)}%"></span></div>
          <div class="metric-note">${ratio > 1 ? 'vCPU 多于物理核，依赖调度复用' : '未超配'}</div>
        </div>
        <div>
          <div class="metric-label">内存合计</div>
          <div class="metric-value">${mem.toFixed(1)} GiB</div>
          <div class="metric-note">宿主总量不由控制面导出</div>
        </div>
        <div>
          <div class="metric-label">进入 guest</div>
          <div class="metric-value">${entry}</div>
          <div class="metric-note">次，累计</div>
        </div>
      </div>
    </div>

    <div class="list-head">
      <span></span><span>客户机</span><span>状态</span><span>vCPU</span><span>内存</span><span></span>
    </div>
    ${rows}

    <section class="section">
      <div class="section-head">
        <h2>物理核占用</h2>
        <p>悬停客户机行可高亮它占用的核</p>
      </div>
      <div class="slots">${lanes}${hostLane}</div>
      <div class="legend">
        <span><i></i>该客户机 pinned</span>
        <span><i class="host"></i>未被任何客户机占用</span>
      </div>
    </section>
  </div>`;
}

function viewHost() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const hh = String(Math.floor(up / 3600)).padStart(2, '0');
  const mm = String(Math.floor((up % 3600) / 60)).padStart(2, '0');
  const ss = String(up % 60).padStart(2, '0');
  return `<div class="view view-enter">
    <div class="page-head">
      <div>
        <h1 class="page-title">宿主机</h1>
        <p class="page-sub">控制面运行其上的这台机器</p>
      </div>
    </div>

    <section class="section" style="margin-top:0">
      <div class="section-head"><h2>系统</h2></div>
      <div class="rows">
        <div class="row"><dt>版本</dt><dd>${HOST.version}</dd></div>
        <div class="row"><dt>架构</dt><dd>${HOST.arch}</dd></div>
        <div class="row"><dt>平台</dt><dd>${HOST.platform}</dd></div>
        <div class="row"><dt>物理核</dt><dd>${HOST.cpus}<small>客户机可 pinned 的核数上限</small></dd></div>
        <div class="row"><dt>SMP</dt><dd>${HOST.smp}</dd></div>
      </div>
    </section>

    <section class="section">
      <div class="section-head"><h2>运行状况</h2></div>
      <div class="rows">
        <div class="row"><dt>运行时长</dt><dd>${hh}:${mm}:${ss}</dd></div>
        <div class="row"><dt>控制台地址</dt><dd>${HOST.web}</dd></div>
        <div class="row"><dt>监听</dt><dd>${HOST.listen}</dd></div>
        <div class="row"><dt>事件通道</dt><dd>${HOST.events}</dd></div>
        <div class="row"><dt>配置池</dt><dd>${HOST.pool.join('  ')}<small>目录按最大深度 8 层扫描</small></dd></div>
        <div class="row"><dt>内存</dt><dd>未导出<small>宿主内存总量不由控制面提供，界面不做估算</small></dd></div>
      </div>
    </section>

    <section class="section">
      <div class="section-head"><h2>构建特性</h2></div>
      <div class="chips">${HOST.features.map((f) => `<span class="chip">${f}</span>`).join('')}</div>
    </section>
  </div>`;
}

function viewConsole() {
  return `<div class="view view-enter">
    <div class="page-head">
      <div>
        <h1 class="page-title">网页控制台</h1>
        <p class="page-sub">vm-1 · arceos-shell · /ws/console/vm-1 独占通道</p>
      </div>
      <div class="page-actions">
        <button class="btn" id="replay">${icon('i-refresh', 14)} 重放</button>
        <button class="btn btn-primary">${icon('i-terminal', 14)} 连接</button>
      </div>
    </div>
    <div class="term">
      <div class="term-bar">
        <span>ttyS0 · 115200 8N1</span>
        <span class="spacer"></span>
        <span class="mono" id="termState">已连接</span>
      </div>
      <div class="term-body" id="termBody"></div>
    </div>
  </div>`;
}

function viewFiles() {
  return `<div class="view view-enter">
    <div class="page-head">
      <div>
        <h1 class="page-title">文件传输</h1>
        <p class="page-sub">写入客户机文件系统前先落到暂存区</p>
      </div>
      <div class="page-actions">
        <button class="btn btn-primary">${icon('i-plus', 14)} 选择文件</button>
      </div>
    </div>
    <div class="list-head" style="grid-template-columns: minmax(0,1fr) 96px 132px 84px">
      <span></span><span>大小</span><span>进度</span><span>状态</span>
    </div>
    <div id="xfers"></div>
  </div>`;
}

function viewShell() {
  return `<div class="view view-enter">
    <div class="page-head">
      <div>
        <h1 class="page-title">Shell</h1>
        <p class="page-sub">控制面所在机器的命令通道 · /ws/shell</p>
      </div>
    </div>
    <div class="term">
      <div class="term-bar">
        <span>axvisor-sh</span>
        <span class="spacer"></span>
        <span class="mono">单会话</span>
      </div>
      <div class="term-body" id="shBody">AxVisor shell — 输入 help 查看可用命令
</div>
      <div class="term-bar">
        <span class="mono" style="color:var(--accent)">$</span>
        <input id="shInput" style="flex:1;background:none;border:0;color:var(--text);font:inherit;font-family:var(--font-data);font-size:var(--text-sm);outline:none" placeholder="输入命令后回车" />
      </div>
    </div>
  </div>`;
}

const mini = (ic, act, label) =>
  `<button class="mini" data-act="${act}" title="${label}" aria-label="${label}">${icon(ic, 14)}</button>`;

/* ---------- render ---------- */
function render() {
  renderNav();
  const main = $('#main');
  const map = { vms: viewVms, host: viewHost, console: viewConsole, files: viewFiles, shell: viewShell };
  main.innerHTML = map[state.view]();
  if (state.view === 'console') startConsole();
  if (state.view === 'files') renderTransfers();
  if (state.view === 'shell') bindShell();
}

function runCmd(cmd) {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const f = (n) => String(n).padStart(2, '0');
  switch (cmd.split(/\s+/)[0]) {
    case 'help':
      return { text: '可用命令：help  uname  free  uptime  ls  vms' };
    case 'uname':
      return { text: `AxVisor ${HOST.version} ${HOST.platform}` };
    case 'free':
      return { text: `客户机已分配 ${VMS.reduce((a, v) => a + v.mem, 0).toFixed(1)} GiB · 宿主总量未导出` };
    case 'uptime':
      return { text: `up ${f(Math.floor(up / 3600))}:${f(Math.floor((up % 3600) / 60))}:${f(up % 60)}` };
    case 'ls':
      return { text: 'arceos-x86_64.img  linux-x86_64.img  rtos-x86_64.img  net-x86_64.img  bench-x86_64.img' };
    case 'vms':
      return { text: VMS.map((v) => `${v.id}  ${v.name.padEnd(14)}${STATE_CN[v.status]}`).join('\n') };
    case '':
      return { text: '' };
    default:
      return { text: `axvisor-sh: 未找到命令 ${cmd}`, err: true };
  }
}

function bindShell() {
  const input = $('#shInput');
  const body = $('#shBody');
  input.focus();
  input.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    const cmd = input.value.trim();
    input.value = '';
    body.insertAdjacentHTML('beforeend', `<div><span class="ts">$</span> ${esc(cmd)}</div>`);
    const out = runCmd(cmd);
    if (out.text) {
      body.insertAdjacentHTML('beforeend', `<div class="${out.err ? 'err' : ''}">${esc(out.text).replace(/\n/g, '<br />')}</div>`);
    }
    body.scrollTop = body.scrollHeight;
  });
}

function renderTransfers() {
  $('#xfers').innerHTML = TRANSFERS.map(
    (t) => `<div class="xfer">
      <span class="name">${esc(t.name)}</span>
      <span class="sz">${bytes(t.total)}</span>
      <span class="bar" style="height:3px"><i style="width:${Math.round((t.done / t.total) * 100)}%;background:${t.done === t.total ? 'var(--ok)' : 'var(--accent-dim)'}"></i></span>
      <span class="pct">${t.state}</span>
    </div>`
  ).join('');
}

/* ---------- console stream ---------- */
let logTimer = null;
function startConsole() {
  const body = $('#termBody');
  body.innerHTML = '';
  let i = 0;
  clearInterval(logTimer);
  logTimer = setInterval(() => {
    if (i >= BOOTLOG.length) {
      clearInterval(logTimer);
      $('#termState').textContent = '已连接 · 空闲';
      return;
    }
    const line = BOOTLOG[i++];
    const cls = line.startsWith('VMM') || line.includes('entering guest') ? 'hi' : '';
    body.insertAdjacentHTML('beforeend', `<div class="${cls}"><span class="ts">[${new Date().toLocaleTimeString('zh-CN', { hour12: false })}]</span> ${esc(line)}</div>`);
    body.scrollTop = body.scrollHeight;
  }, 260);
}

/* ---------- drawer ---------- */
function openDrawer(id) {
  const v = VMS.find((x) => x.id === id);
  if (!v) return;
  state.selected = id;
  const acts =
    v.status === 'Running'
      ? `<button class="btn" data-act="pause">${icon('i-pause', 14)} 暂停</button><button class="btn btn-danger" data-act="stop">${icon('i-stop', 14)} 停止</button>`
      : `<button class="btn btn-primary" data-act="start">${icon('i-play', 14)} 启动</button><button class="btn btn-danger" data-act="destroy">${icon('i-x', 14)} 关闭</button>`;
  const wrap = document.createElement('div');
  wrap.className = 'drawer-wrap';
  wrap.innerHTML = `<div class="scrim" data-close></div>
    <aside class="drawer" role="dialog" aria-label="${esc(v.name)} 详情">
      <div class="drawer-head">
        <div style="flex:1">
          <h2>${esc(v.name)}</h2>
          <p>${v.id} · ${esc(v.image)}</p>
        </div>
        <button class="mini" data-close aria-label="关闭">${icon('i-x', 16)}</button>
      </div>
      <div class="stat-pair">
        <div><span>进入 guest</span><b>${v.entry}</b></div>
        <div><span>park 次数</span><b>${v.park}</b></div>
      </div>
      <div class="rows" style="border-top:0">
        <div class="row"><dt>状态</dt><dd>${STATE_CN[v.status]}${v.issue ? `<small>异常：${v.issue}</small>` : ''}</dd></div>
        <div class="row"><dt>vCPU</dt><dd>${v.vcpus}</dd></div>
        <div class="row"><dt>亲和</dt><dd>${v.cpus.join(' ')}<small>pCPU 编号，共 ${PCPUS} 个核</small></dd></div>
        <div class="row"><dt>内存</dt><dd>${memStr(v.mem)}</dd></div>
        <div class="row"><dt>内核参数</dt><dd>${v.cmdline || '—'}</dd></div>
      </div>
      <section class="section">
        <div class="section-head"><h2>核占用</h2></div>
        <div class="lane">${Array.from({ length: PCPUS }, (_, i) => `<span class="cell ${v.cpus.includes(i) ? 'on' : ''}"></span>`).join('')}</div>
      </section>
      <div class="page-actions" style="margin-top:var(--s-5)">${acts}</div>
    </aside>`;
  document.body.appendChild(wrap);
}
function closeDrawer() {
  $$('.drawer-wrap').forEach((el) => el.remove());
  state.selected = null;
}

/* 客户机行悬停时高亮它在物理核上占用的槽位 */
document.addEventListener('mouseover', (e) => {
  const row = e.target.closest('[data-open]');
  $$('.slot-row').forEach((r) => r.classList.toggle('hot', !!row && r.dataset.lane === row.dataset.open));
});

/* ---------- command palette ---------- */
function cmdkItems() {
  const views = VIEWS.map((v) => ({ label: `前往 ${v.label}`, run: () => switchView(v.id) }));
  const vms = VMS.map((v) => ({ label: `查看 ${v.name}`, hint: v.id, run: () => { switchView('vms'); openDrawer(v.id); } }));
  return [...views, ...vms];
}

function openCmdk() {
  state.cmdkOpen = true;
  state.query = '';
  state.cmdkIndex = 0;
  const wrap = document.createElement('div');
  wrap.className = 'cmdk-wrap';
  wrap.innerHTML = `<div class="cmdk">
    <input id="cmdkInput" placeholder="搜索视图或客户机" aria-label="搜索" />
    <div class="cmdk-list" id="cmdkList"></div>
  </div>`;
  wrap.addEventListener('click', (e) => {
    if (e.target === wrap) closeCmdk();
  });
  document.body.appendChild(wrap);
  $('#cmdkInput').focus();
  $('#cmdkInput').addEventListener('input', (e) => {
    state.query = e.target.value;
    state.cmdkIndex = 0;
    renderCmdk();
  });
  $('#cmdkInput').addEventListener('keydown', onCmdkKey);
  renderCmdk();
}

function renderCmdk() {
  const items = cmdkItems().filter((i) => i.label.toLowerCase().includes(state.query.toLowerCase()));
  const list = $('#cmdkList');
  if (!list) return;
  if (!items.length) {
    list.innerHTML = `<div class="cmdk-empty">没有匹配项</div>`;
    return;
  }
  list.innerHTML = items
    .map(
      (i, n) => `<button class="cmdk-item" data-cmdk="${n}" aria-selected="${n === state.cmdkIndex}">
      <span>${esc(i.label)}</span>${i.hint ? `<kbd>${i.hint}</kbd>` : ''}
    </button>`
    )
    .join('');
  $$('#cmdkList .cmdk-item').forEach((b) =>
    b.addEventListener('click', () => {
      const filtered = cmdkItems().filter((i) => i.label.toLowerCase().includes(state.query.toLowerCase()));
      closeCmdk();
      filtered[Number(b.dataset.cmdk)].run();
    })
  );
}

function onCmdkKey(e) {
  const items = cmdkItems().filter((i) => i.label.toLowerCase().includes(state.query.toLowerCase()));
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    state.cmdkIndex = (state.cmdkIndex + 1) % Math.max(1, items.length);
    renderCmdk();
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    state.cmdkIndex = (state.cmdkIndex - 1 + items.length) % Math.max(1, items.length);
    renderCmdk();
  } else if (e.key === 'Enter') {
    e.preventDefault();
    const it = items[state.cmdkIndex];
    if (it) {
      closeCmdk();
      it.run();
    }
  } else if (e.key === 'Escape') {
    closeCmdk();
  }
}

function closeCmdk() {
  $$('.cmdk-wrap').forEach((el) => el.remove());
  state.cmdkOpen = false;
}

/* ---------- theme ---------- */
function setTheme(t) {
  state.theme = t;
  document.documentElement.dataset.theme = t;
  localStorage.setItem('axvisor.theme', t);
  $$('[data-theme-btn]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.themeBtn === t)));
}

function switchView(v) {
  state.view = v;
  clearInterval(logTimer);
  closeDrawer();
  render();
}

/* ---------- events ---------- */
document.addEventListener('click', (e) => {
  const nav = e.target.closest('[data-view]');
  if (nav) return switchView(nav.dataset.view);

  const themeBtn = e.target.closest('[data-theme-btn]');
  if (themeBtn) return setTheme(themeBtn.dataset.themeBtn);

  if (e.target.closest('#cmdOpen')) return openCmdk();
  if (e.target.closest('#refresh') || e.target.closest('#replay')) {
    if (state.view === 'console') startConsole();
    return render();
  }
  if (e.target.closest('[data-close]')) return closeDrawer();

  const act = e.target.closest('[data-act]');
  if (act) {
    e.stopPropagation();
    return;
  }

  const row = e.target.closest('[data-open]');
  if (row) return openDrawer(row.dataset.open);

  if (e.target.classList.contains('scrim')) closeDrawer();
});

document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    state.cmdkOpen ? closeCmdk() : openCmdk();
    return;
  }
  if (e.key === 'Escape' && state.selected) closeDrawer();
  if (e.key === 'Enter' && e.target.matches('.guest')) openDrawer(e.target.dataset.open);
});

/* ---------- status bar ---------- */
function tick() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const f = (n) => String(n).padStart(2, '0');
  $('#sbUptime').textContent = `运行 ${f(Math.floor(up / 3600))}:${f(Math.floor((up % 3600) / 60))}:${f(up % 60)}`;
  $('#sbClock').textContent = new Date().toLocaleTimeString('zh-CN', { hour12: false });
}

/* ---------- boot ---------- */
$('#ver').textContent = HOST.version;
$('#hlArch').textContent = HOST.arch;
$('#hlPlat').textContent = HOST.platform;
$('#hlCpu').textContent = `${HOST.cpus} pCPU`;
$('#sbPool').textContent = `配置池 ${HOST.pool[0]}`;
$('#sbEv').textContent = `事件通道 ${HOST.events}`;
setTheme(state.theme);
render();
tick();
setInterval(tick, 1000);
setInterval(() => {
  if (state.view === 'files') {
    const t = TRANSFERS.find((x) => x.state === '传输中');
    if (t && t.done < t.total) {
      t.done = Math.min(t.total, t.done + t.total * 0.06);
      if (t.done >= t.total) t.state = '完成';
      renderTransfers();
    }
  }
}, 700);
