/* AxVisor 控制台 v4 — 数据形状沿用真实控制面契约。 */

const PCPUS = 8;
const BOOT = Date.now() - (3 * 3600 + 42 * 60) * 1000;

const HOST = {
  version: '0.8.2',
  arch: 'x86_64',
  platform: 'x86_64-qemu-q35',
  cpus: PCPUS,
  web: 'http://10.0.2.15:8080',
  listen: '0.0.0.0:8080',
  features: ['fs', 'http-axum', 'browser-console', 'web-ui'],
  pool: ['/guest', 'AXVISOR_VM_DIRS'],
};

const VMS = [
  { id: 'vm-1', name: 'arceos-shell', status: 'Running', vcpus: 4, mem: 1.0, cpus: [0, 1, 2, 3], entry: 12, park: 3, image: 'arceos-x86_64.img', cmdline: 'LOG=info' },
  { id: 'vm-2', name: 'linux-guest', status: 'Running', vcpus: 2, mem: 2.0, cpus: [4, 5], entry: 8, park: 1, image: 'linux-x86_64.img', cmdline: 'console=ttyS0' },
  { id: 'vm-3', name: 'rtos-node', status: 'Paused', vcpus: 2, mem: 0.5, cpus: [0, 1], entry: 5, park: 5, image: 'rtos-x86_64.img', cmdline: '' },
  { id: 'vm-4', name: 'net-peer', status: 'Stopped', vcpus: 2, mem: 0.5, cpus: [6, 7], entry: 0, park: 0, image: 'net-x86_64.img', cmdline: '' },
  { id: 'vm-5', name: 'blk-bench', status: 'Failed', vcpus: 1, mem: 4.0, cpus: [7], entry: 2, park: 0, image: 'bench-x86_64.img', cmdline: '', issue: 'duplicate-id' },
];

const STATE_CN = { Running: '运行中', Paused: '已暂停', Stopped: '已停止', Failed: '失败' };

const VIEWS = [
  { id: 'vms', label: '客户机', icon: 'i-server', count: () => VMS.length },
  { id: 'console', label: '网页控制台', icon: 'i-terminal' },
  { id: 'files', label: '文件传输', icon: 'i-file' },
  { id: 'shell', label: 'Shell', icon: 'i-shell' },
];
const SYS_VIEWS = [{ id: 'host', label: '宿主机', icon: 'i-cpu' }];

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
  'VMM setup done, entering guest',
  '[    0.112] ArceOS starting...',
  '[    0.118] axhal: x86_64 initialized',
  '[    0.124] axalloc: bitmap allocator ready',
  '[    0.131] axtask: 4 tasks spawned',
  '[    0.140] axfs: mounting ramfs at /',
  '[    0.152] shell: arceos shell ready',
];

/* 这台机器上持续发生的事件 —— 界面"活着"的来源 */
const EVENTS = [
  'vm-1 vcpu 2 → VM-Entry (rip 0x8000)',
  'vm-2 virtio-net rx 42 packets',
  'vm-1 vcpu 0 → VM-Exit reason=30',
  'vm-2 block I/O completed 8 sectors',
  'vm-3 paused: vcpu parked',
  'vm-1 vcpu 3 → VM-Entry',
  'vm-2 virtio-net tx 17 packets',
  'vm-1 external interrupt delivered',
  'vm-5 load refused: duplicate-id',
  'vm-2 timer fired (deadline +10ms)',
];

/* ---------- helpers ---------- */
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
const icon = (id, size = 15) =>
  `<svg viewBox="0 0 24 24" width="${size}" height="${size}"><use href="#${id}" /></svg>`;
const memStr = (g) => (g >= 1 ? `${g.toFixed(1)} GiB` : `${Math.round(g * 1024)} MiB`);
const bytes = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`);

const holds = (vm) => vm.status === 'Running' || vm.status === 'Paused';
const occupancy = () =>
  Array.from({ length: PCPUS }, (_, cpu) => VMS.filter((v) => holds(v) && v.cpus.includes(cpu)));
const usedCpus = () => occupancy().filter((o) => o.length > 0).length;
const totalVcpus = () => VMS.reduce((n, v) => n + v.vcpus, 0);
const totalMem = () => VMS.reduce((n, v) => n + v.mem, 0);
const runningCount = () => VMS.filter((v) => v.status === 'Running').length;

const state = {
  view: 'vms',
  theme: localStorage.getItem('axvisor.theme') || 'stage',
  hover: null,
  cmdkOpen: false,
  cmdkIndex: 0,
  query: '',
};

/* ---------- 顶栏 ---------- */
function renderChips() {
  $('#chipArch').textContent = HOST.arch;
  $('#chipCpus').textContent = `${PCPUS} 物理核`;
  $('#chipLoad').textContent = `${usedCpus()} 核已划分`;
  $('#sbAddr').textContent = HOST.listen;
}

/* ---------- 侧栏 ---------- */
function renderNav() {
  const group = (title, items) =>
    `<div class="rail-group"><div class="label">${title}</div>${items}</div>`;
  const item = (v) =>
    `<button class="nav-item" data-view="${v.id}" aria-current="${state.view === v.id}">
      ${icon(v.icon)}<span>${v.label}</span>
      ${v.count ? `<span class="nav-count">${v.count()}</span>` : ''}
    </button>`;
  $('#nav').innerHTML =
    group('能力', VIEWS.map(item).join('')) + group('系统', SYS_VIEWS.map(item).join(''));
  $$('#nav .nav-item').forEach((b) => b.addEventListener('click', () => switchView(b.dataset.view)));
}

/* ---------- 主张 + 舞台 ---------- */
function viewVms() {
  const occ = occupancy();
  const used = occ.filter((o) => o.length).length;
  const ratio = (totalVcpus() / PCPUS).toFixed(1);

  /* 每个物理核一条光带；被分时共享时拆成两条更细的 */
  const lanes = occ
    .map((owners, cpu) => {
      const busy = owners.length > 0;
      const inner = busy ? owners.map(() => '<i></i>').join('') : '';
      const cls = [busy ? '' : 'idle', owners.length > 1 ? 'shared' : ''].filter(Boolean).join(' ');
      return `<div class="lane ${cls}">
        <div class="bar">${inner}</div>
        <div class="lane-foot"><span>${cpu}</span><span>${
          busy ? (owners.length > 1 ? '分时' : esc(owners[0].name)) : '空闲'
        }</span></div>
      </div>`;
    })
    .join('');

  const memSegs = VMS.filter(holds)
    .map(
      (v) =>
        `<i style="width:${((v.mem / totalMem()) * 100).toFixed(3)}%" title="${esc(v.name)} ${memStr(v.mem)}"></i>`
    )
    .join('');

  const rows = VMS.map(
    (v) => `<div class="row" data-vm="${v.id}" tabindex="0">
      <span class="name">${esc(v.name)}</span>
      <span class="state ${v.status.toLowerCase()}"><span class="dot"></span>${STATE_CN[v.status]}</span>
      <span class="cell">${v.vcpus} vCPU · ${memStr(v.mem)}</span>
      <span class="cell cpus">${v.cpus.join(', ')}</span>
      <span class="ops">
        <button class="icon-btn" data-act="start" title="启动">${icon('i-play', 14)}</button>
        <button class="icon-btn" data-act="stop" title="停止">${icon('i-stop', 14)}</button>
        <button class="icon-btn danger" data-act="del" title="销毁">${icon('i-trash', 14)}</button>
      </span>
    </div>`
  ).join('');

  return `
  <section class="hero rise" style="--i:0">
    <p class="hero-stmt">
      <span class="num" data-count="${runningCount()}">0</span> 台客户机正在运行<span class="dim">，</span><br />
      <span class="dim">共用</span> <span class="num" data-count="${PCPUS}">0</span> 个物理核。
    </p>
    <div class="hero-sub">
      <span>已划分 <span class="k">${used}/${PCPUS}</span></span>
      <span class="sep">·</span>
      <span>超配比 <span class="k">${ratio}×</span></span>
      <span class="sep">·</span>
      <span>内存 <span class="k">${memStr(totalMem())}</span></span>
      <span class="sep">·</span>
      <span>${esc(HOST.platform)}</span>
    </div>

    <div class="stage">
      <div class="stage-head">
        <span class="t">物理核划分</span>
        <span class="m">${PCPUS} × ${esc(HOST.arch)}</span>
        <span class="r">
          <div><span class="n">${used}<span style="font-size:13px;color:var(--ink-4)">/${PCPUS}</span></span><span class="label">已划分</span></div>
          <div><span class="n">${ratio}<span style="font-size:13px;color:var(--ink-4)">×</span></span><span class="label">超配比</span></div>
          <div><span class="n">${PCPUS - used}</span><span class="label">空闲核</span></div>
        </span>
      </div>

      <div class="track" id="track">${lanes}</div>

      <div class="memlane">${memSegs}</div>
      <div class="stage-note">
        已分配 <span class="mono">${memStr(totalMem())}</span> 内存
        <span class="q">· 宿主总量不由控制面导出，界面不做估算</span>
      </div>
    </div>
  </section>

  <section class="section rise" style="--i:1">
    <div class="section-head">
      <span class="title">客户机</span>
      <span class="count">${VMS.length}</span>
    </div>
    <div class="rows">${rows}</div>
  </section>`;
}

/* ---------- 其它视图 ---------- */
function viewConsole() {
  return `<div class="term">
    <div class="term-head">
      <span class="t">串口控制台</span>
      <span class="m">arceos-shell · /ws/console/vm-1</span>
      <span style="margin-left:auto"></span>
      <button class="btn">断开</button>
    </div>
    <div class="term-body" id="termBody"></div>
  </div>`;
}

function viewFiles() {
  return `<div class="pane">
    <h2>文件传输</h2>
    <div class="sub">暂存区中的文件，可写入客户机文件系统。</div>
    ${TRANSFERS.map(
      (t) => `<div class="xfer${t.state === '传输中' ? ' active' : ''}">
        <div>
          <div style="font-size:13.5px">${esc(t.name)}</div>
          <div class="bar"><i style="width:${((t.done / t.total) * 100).toFixed(1)}%"></i></div>
        </div>
        <span class="st">${t.state}</span>
        <span class="pct">${bytes(t.done)} / ${bytes(t.total)}</span>
      </div>`
    ).join('')}
  </div>`;
}

function viewShell() {
  return `<div class="pane">
    <h2>Shell</h2>
    <div class="sub">在 Hypervisor 上直接执行命令。</div>
    <div class="term-body" id="shBody" style="border:1px solid var(--line-soft);border-radius:7px;height:300px;padding:16px 18px">
      <div><span class="ts">$</span> <span class="hi">uname -a</span></div>
      <div style="color:var(--ink-2)">AxVisor 0.8.2 x86_64-qemu-q35</div>
      <div><span class="ts">$</span> <span class="hi">help</span></div>
      <div style="color:var(--ink-2)">可用命令：help · version · uname · ps · free · vm</div>
      <div><span class="ts">$</span> <span class="caret" style="display:inline-block;width:6px;height:12px;background:var(--accent);vertical-align:-2px"></span></div>
    </div>
  </div>`;
}

function viewHost() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const h = Math.floor(up / 3600);
  const m = Math.floor((up % 3600) / 60);
  const kv = (k, v) => `<div class="kv"><span class="k">${k}</span><span class="v">${v}</span></div>`;
  return `<div class="pane">
    <h2>宿主机</h2>
    <div class="sub">运行 AxVisor 的物理机器。</div>
    ${kv('版本', HOST.version)}
    ${kv('架构', HOST.arch)}
    ${kv('平台', HOST.platform)}
    ${kv('物理核', PCPUS)}
    ${kv('运行时长', `${h} 小时 ${m} 分`)}
    ${kv('Web 控制台', HOST.web)}
    ${kv('监听地址', HOST.listen)}
    ${kv('构建特性', `<span class="chips">${HOST.features.map((f) => `<span class="chip">${f}</span>`).join('')}</span>`)}
    ${kv('配置池', HOST.pool.map((p) => `<span class="chip">${esc(p)}</span>`).join(' '))}
    ${kv('已划分核', `${usedCpus()} / ${PCPUS}`)}
    ${kv('客户机 vCPU 合计', `${totalVcpus()}（超配比 ${(totalVcpus() / PCPUS).toFixed(1)}×）`)}
    ${kv('已分配内存', memStr(totalMem()))}
    ${kv('宿主内存总量', '<span style="color:var(--ink-3)">不由控制面导出</span>')}
  </div>`;
}

const RENDER = { vms: viewVms, console: viewConsole, files: viewFiles, shell: viewShell, host: viewHost };

/* 高频动作：切换视图不加动画 */
function switchView(id) {
  if (!RENDER[id]) return;
  state.view = id;
  state.hover = null;
  $$('#nav .nav-item').forEach((b) => b.setAttribute('aria-current', String(b.dataset.view === id)));
  $('#workspace').innerHTML = RENDER[id]();
  bindWorkspace();
}

function bindWorkspace() {
  renderChips();
  countUp();
  $$('.row').forEach((row) => {
    const id = row.dataset.vm;
    row.addEventListener('click', (e) => {
      if (e.target.closest('[data-act]')) return;
      openDrawer(id);
    });
    row.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') openDrawer(id);
    });
    row.addEventListener('mouseenter', () => focusVm(id));
    row.addEventListener('mouseleave', () => focusVm(null));
  });
  $$('[data-act]').forEach((b) => b.addEventListener('click', (e) => e.stopPropagation()));
  if (state.view === 'console') startConsole();
  if (state.view === 'files') tickTransfers();
}

/* 悬停客户机：它的核亮起，其余退让 */
function focusVm(id) {
  state.hover = id;
  const track = $('#track');
  if (!track) return;
  track.classList.toggle('focusing', !!id);
  $$('.lane', track).forEach((lane, cpu) => {
    const owners = occupancy()[cpu];
    lane.classList.toggle('lit', !!id && owners.some((v) => v.id === id));
  });
}

/* 数字滚动：一次性的入场质感 */
function countUp() {
  $$('[data-count]').forEach((el) => {
    const to = Number(el.dataset.count);
    const dur = 620;
    const t0 = performance.now();
    const tick = (t) => {
      const p = Math.min(1, (t - t0) / dur);
      const eased = 1 - Math.pow(1 - p, 3);
      el.textContent = Math.round(to * eased);
      if (p < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

/* ---------- 抽屉 ---------- */
function openDrawer(id) {
  const v = VMS.find((x) => x.id === id);
  if (!v) return;
  const wrap = document.createElement('div');
  wrap.innerHTML = `<div class="scrim" data-close></div>
    <aside class="drawer" role="dialog" aria-label="${esc(v.name)}">
      <div class="drawer-head">
        <div style="flex:1">
          <div class="t">${esc(v.name)}</div>
          <div class="s"><span class="state ${v.status.toLowerCase()}"><span class="dot"></span>${STATE_CN[v.status]}</span></div>
        </div>
        <button class="icon-btn" data-close aria-label="关闭">${icon('i-close', 15)}</button>
      </div>
      <div class="drawer-body">
        <div class="bignums">
          <div class="bignum"><div class="n">${v.entry}</div><div class="label">进入 guest</div></div>
          <div class="bignum"><div class="n">${v.park}</div><div class="label">park 次数</div></div>
        </div>
        ${[['ID', v.id], ['vCPU', v.vcpus], ['内存', memStr(v.mem)], ['镜像', v.image], ['内核参数', v.cmdline || '—']]
          .map(([k, val]) => `<div class="field"><span class="k">${k}</span><span class="v">${esc(val)}</span></div>`)
          .join('')}
        <div class="field">
          <span class="k">物理核亲和</span>
          <span class="v"><span class="pin-grid">${Array.from(
            { length: PCPUS },
            (_, i) => `<span class="pin${v.cpus.includes(i) ? ' on' : ''}">${i}</span>`
          ).join('')}</span></span>
        </div>
        ${v.issue ? `<div class="notice">配置问题：<span class="mono">${esc(v.issue)}</span> — 该客户机的 ID 与池中另一项重复，未加载。</div>` : ''}
      </div>
    </aside>`;
  document.body.appendChild(wrap);
  requestAnimationFrame(() => {
    $('.scrim', wrap).classList.add('in');
    $('.drawer', wrap).classList.add('in');
  });
  $$('[data-close]', wrap).forEach((el) => el.addEventListener('click', () => closeDrawer(wrap)));
}

function closeDrawer(wrap) {
  $('.scrim', wrap)?.classList.remove('in');
  $('.drawer', wrap)?.classList.remove('in');
  setTimeout(() => wrap.remove(), 420);
}

/* ---------- 命令面板 ---------- */
function openCmdk() {
  if (state.cmdkOpen) return;
  state.cmdkOpen = true;
  state.query = '';
  state.cmdkIndex = 0;
  const wrap = document.createElement('div');
  wrap.className = 'cmdk-wrap';
  wrap.innerHTML = `<div class="cmdk" role="dialog" aria-label="命令面板">
    <input id="cmdkInput" placeholder="跳转到面板或客户机…" />
    <div class="cmdk-list" id="cmdkList"></div>
  </div>`;
  document.body.appendChild(wrap);
  const close = () => {
    wrap.remove();
    state.cmdkOpen = false;
  };
  wrap.addEventListener('click', (e) => {
    if (e.target === wrap) close();
  });
  $('#cmdkInput', wrap).addEventListener('input', (e) => {
    state.query = e.target.value;
    state.cmdkIndex = 0;
    renderCmdk();
  });
  $('#cmdkInput', wrap).addEventListener('keydown', (e) => {
    const items = cmdkItems();
    if (e.key === 'Escape') return close();
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      state.cmdkIndex = (state.cmdkIndex + 1) % items.length;
      renderCmdk();
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      state.cmdkIndex = (state.cmdkIndex - 1 + items.length) % items.length;
      renderCmdk();
    }
    if (e.key === 'Enter') {
      const it = items[state.cmdkIndex];
      if (it) {
        close();
        it.run();
      }
    }
  });
  $('#cmdkInput', wrap).focus();
  renderCmdk();
}

function cmdkItems() {
  const q = state.query.trim().toLowerCase();
  const views = [...VIEWS, ...SYS_VIEWS].map((v) => ({
    icon: v.icon,
    label: v.label,
    hint: '面板',
    run: () => switchView(v.id),
  }));
  const vms = VMS.map((v) => ({
    icon: 'i-server',
    label: v.name,
    hint: STATE_CN[v.status],
    run: () => {
      switchView('vms');
      setTimeout(() => openDrawer(v.id), 60);
    },
  }));
  return [...views, ...vms].filter((i) => !q || i.label.toLowerCase().includes(q));
}

function renderCmdk() {
  const list = $('#cmdkList');
  if (!list) return;
  const items = cmdkItems();
  if (!items.length) {
    list.innerHTML = `<div class="cmdk-empty">没有匹配项</div>`;
    return;
  }
  list.innerHTML = items
    .map(
      (it, i) => `<button class="cmdk-item${i === state.cmdkIndex ? ' on' : ''}" data-i="${i}">
        ${icon(it.icon, 14)}<span>${esc(it.label)}</span><span class="hint">${esc(it.hint)}</span>
      </button>`
    )
    .join('');
  $$('.cmdk-item', list).forEach((b) =>
    b.addEventListener('click', () => {
      const it = items[Number(b.dataset.i)];
      $('.cmdk-wrap')?.remove();
      state.cmdkOpen = false;
      it.run();
    })
  );
}

/* ---------- 串口：逐行流入 ---------- */
let consoleTimer = null;
function startConsole() {
  const body = $('#termBody');
  if (!body) return;
  let i = 0;
  clearInterval(consoleTimer);
  const step = () => {
    if (!document.body.contains(body)) return clearInterval(consoleTimer);
    if (i >= BOOTLOG.length) return clearInterval(consoleTimer);
    const line = BOOTLOG[i++];
    const div = document.createElement('div');
    div.className = 'term-line';
    div.innerHTML = `<span class="ts">${new Date().toLocaleTimeString('zh-CN', { hour12: false })}</span><span class="${line.startsWith('[') ? '' : 'hi'}">${esc(line)}</span>`;
    body.appendChild(div);
    body.scrollTop = body.scrollHeight;
  };
  step();
  consoleTimer = setInterval(step, 240);
}

/* ---------- 传输推进 ---------- */
function tickTransfers() {
  const t = TRANSFERS.find((x) => x.state === '传输中');
  if (!t || t.done >= t.total) return;
  const timer = setInterval(() => {
    t.done = Math.min(t.total, t.done + 1_400_000);
    if (t.done >= t.total) {
      t.state = '完成';
      clearInterval(timer);
    }
    if (state.view === 'files') {
      $('#workspace').innerHTML = viewFiles();
    } else {
      clearInterval(timer);
    }
  }, 700);
}

/* ---------- 底部事件流：逐字，界面因此是"活"的 ---------- */
let evIndex = 0;
function streamEvent() {
  const el = $('#evStream');
  if (!el) return;
  const text = EVENTS[evIndex % EVENTS.length];
  evIndex++;
  const ts = new Date().toLocaleTimeString('zh-CN', { hour12: false });
  let i = 0;
  el.innerHTML = `<span class="ts">${ts}</span><span class="txt"></span><span class="caret"></span>`;
  const txt = $('.txt', el);
  const timer = setInterval(() => {
    txt.textContent = text.slice(0, ++i);
    if (i >= text.length) {
      clearInterval(timer);
      setTimeout(streamEvent, 1600);
    }
  }, 22);
}

function tickUptime() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const h = Math.floor(up / 3600);
  const m = Math.floor((up % 3600) / 60);
  const s = up % 60;
  const el = $('#sbUp');
  if (el) el.textContent = `uptime ${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

/* ---------- 主题 ---------- */
function setTheme(name) {
  state.theme = name;
  document.documentElement.dataset.theme = name;
  localStorage.setItem('axvisor.theme', name);
  $$('#themeSwitch button').forEach((b) => b.classList.toggle('on', b.dataset.theme === name));
}

/* ---------- 启动 ---------- */
renderNav();
$$('#themeSwitch button').forEach((b) => b.addEventListener('click', () => setTheme(b.dataset.theme)));
$('#cmdkBtn').addEventListener('click', openCmdk);
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    openCmdk();
  }
  if (e.key === 'Escape' && state.cmdkOpen) {
    $('.cmdk-wrap')?.remove();
    state.cmdkOpen = false;
  }
});
setTheme(state.theme);
switchView('vms');
streamEvent();
tickUptime();
setInterval(tickUptime, 1000);
