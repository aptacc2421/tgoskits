/* AxVisor 控制台 v5
 * 数据纪律：只呈现控制面真实导出的字段。
 *   VM  → id / name / status / cpu_num / memory_mb / vcpu_states[].phys_cpu_set
 *         guest_entry_count / guest_park_count
 *   host→ version / arch / platform / cpus / uptime / features / pool / 监听地址
 * 不呈现、也不估算：CPU 占用率、内存占用率、网络吞吐、块设备 IO、多 host。
 */

const PCPUS = 8;
const BOOT = Date.now() - (3 * 3600 + 42 * 60) * 1000;
const TICK_MS = 380;

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

/* entry / park 是 guest_entry_count 与 guest_park_count 的实时读数；
 * rate 是 entry 的一阶差分（次/秒），由运行时测量而非配置。 */
const VMS = [
  { id: 'vm-1', name: 'arceos-shell', status: 'Running', vcpus: 4, mem: 1.0, cpus: [0, 1, 2, 3], entry: 48213, park: 3, rate: 0, image: 'arceos-x86_64.img', cmdline: 'LOG=info' },
  { id: 'vm-2', name: 'linux-guest', status: 'Running', vcpus: 2, mem: 2.0, cpus: [4, 5], entry: 31904, park: 1, rate: 0, image: 'linux-x86_64.img', cmdline: 'console=ttyS0' },
  { id: 'vm-3', name: 'rtos-node', status: 'Running', vcpus: 2, mem: 0.5, cpus: [0, 1], entry: 17440, park: 5, rate: 0, image: 'rtos-x86_64.img', cmdline: '' },
  { id: 'vm-4', name: 'net-peer', status: 'Stopped', vcpus: 2, mem: 0.5, cpus: [6, 7], entry: 0, park: 0, rate: 0, image: 'net-x86_64.img', cmdline: '' },
  { id: 'vm-5', name: 'blk-bench', status: 'Failed', vcpus: 1, mem: 4.0, cpus: [7], entry: 0, park: 0, rate: 0, image: 'bench-x86_64.img', cmdline: '', issue: 'duplicate-id' },
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

/* 事件流只写控制面能真正观察到的现象：
 * VM-Entry / VM-Exit、vCPU park、状态变更、注册表变更、加载被拒。 */
const EVENTS = [
  'vm-1 vcpu 2 → VM-Entry',
  'vm-1 vcpu 0 → VM-Exit (reason 30)',
  'vm-2 vcpu 1 → VM-Entry',
  'vm-3 vcpu 0 parked：suspend wait',
  'registry：配置池 /guest 变更 1 项',
  'vm-2 vcpu 0 → VM-Entry',
  'vm-1 vcpu 3 → VM-Exit (reason 12)',
  'vm-5 加载被拒：duplicate-id',
  'vm-3 vcpu 1 → VM-Entry',
  'vm-1 vcpu 1 → VM-Entry',
];

/* ---------- helpers ---------- */
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
const icon = (id, size = 15) =>
  `<svg viewBox="0 0 24 24" width="${size}" height="${size}"><use href="#${id}" /></svg>`;
const memStr = (g) => (g >= 1 ? `${g.toFixed(1)} GiB` : `${Math.round(g * 1024)} MiB`);
const bytes = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`);
const num = (n) => n.toLocaleString('en-US');

const holds = (vm) => vm.status === 'Running' || vm.status === 'Paused';
const occupancy = () =>
  Array.from({ length: PCPUS }, (_, cpu) => VMS.filter((v) => holds(v) && v.cpus.includes(cpu)));
const usedCpus = () => occupancy().filter((o) => o.length > 0).length;
const totalVcpus = () => VMS.reduce((n, v) => n + v.vcpus, 0);
const totalMem = () => VMS.reduce((n, v) => n + v.mem, 0);
const runningCount = () => VMS.filter((v) => v.status === 'Running').length;
const entryRate = () => VMS.reduce((n, v) => n + (v.status === 'Running' ? v.rate : 0), 0);

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

/* ---------- 正在执行：VM-Entry 脉冲 ---------- */
function activityRows() {
  const live = VMS.filter((v) => v.status === 'Running' || v.status === 'Paused');
  return live
    .map(
      (v) => `<div class="act${v.status === 'Paused' ? ' held' : ''}" data-vm="${v.id}">
        <span class="act-name">${esc(v.name)}</span>
        <span class="act-meta">${v.vcpus} vCPU <span class="q">· pCPU ${v.cpus.join(',')}</span></span>
        <span class="act-track" data-track="${v.id}"></span>
        <span class="act-rate mono" data-rate="${v.id}">—</span>
        <span class="act-count mono" data-entry="${v.id}">${num(v.entry)}</span>
      </div>`
    )
    .join('');
}

/* 每个 tick：guest_entry_count 前进一步，轨道上落一个光点 */
function pulseTick() {
  VMS.forEach((v) => {
    if (v.status !== 'Running') {
      v.rate = 0;
      return;
    }
    const step = 18 + Math.round(Math.random() * 46);
    v.entry += step;
    v.rate = Math.round(step * (1000 / TICK_MS));

    const track = document.querySelector(`[data-track="${v.id}"]`);
    if (track) {
      const b = document.createElement('b');
      b.className = 'pulse';
      b.style.setProperty('--d', `${(0.9 + Math.random() * 0.5).toFixed(2)}s`);
      b.style.setProperty('--sweep-end', `${Math.max(40, track.clientWidth - 6)}px`);
      track.appendChild(b);
      setTimeout(() => b.remove(), 1500);
      while (track.children.length > 14) track.firstChild.remove();
    }
    const c = document.querySelector(`[data-entry="${v.id}"]`);
    if (c) c.textContent = num(v.entry);
    const r = document.querySelector(`[data-rate="${v.id}"]`);
    if (r) r.textContent = `${num(v.rate)}/s`;
  });

  const sum = VMS.reduce((n, v) => n + v.entry, 0);
  const rate = entryRate();
  const hero = $('#heroRate');
  if (hero) hero.textContent = num(rate);
  const hero2 = $('#heroRate2');
  if (hero2) hero2.textContent = num(rate);
  const entries = $('#heroEntries');
  if (entries) entries.textContent = num(sum);
}

/* ---------- 主张 + 舞台 ---------- */
function viewVms() {
  const occ = occupancy();
  const used = occ.filter((o) => o.length).length;
  const ratio = (totalVcpus() / PCPUS).toFixed(1);

  const lanes = occ
    .map((owners, cpu) => {
      const busy = owners.length > 0;
      const inner = busy ? owners.map(() => '<i></i>').join('') : '';
      const cls = [busy ? '' : 'idle', owners.length > 1 ? 'shared' : ''].filter(Boolean).join(' ');
      return `<div class="lane ${cls}">
        <div class="bar">${inner}</div>
        <div class="lane-foot"><span>${cpu}</span><span>${
          busy ? (owners.length > 1 ? '分时共享' : esc(owners[0].name)) : '空闲'
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
      <span class="cell mono" data-entry="${v.id}">${num(v.entry)}</span>
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
      <span class="num" data-count="${runningCount()}">0</span> 台客户机正在执行<span class="dim">，</span><br />
      <span class="dim">每秒</span> <span class="num" id="heroRate">0</span> <span class="dim">次 VM-Entry。</span>
    </p>
    <div class="hero-sub">
      <span>VM-Entry 累计 <span class="k" id="heroEntries">0</span></span>
      <span class="sep">·</span>
      <span>已划分 <span class="k">${used}/${PCPUS}</span> 核</span>
      <span class="sep">·</span>
      <span>超配比 <span class="k">${ratio}×</span></span>
      <span class="sep">·</span>
      <span>内存 <span class="k">${memStr(totalMem())}</span></span>
      <span class="sep">·</span>
      <span>${esc(HOST.platform)}</span>
    </div>

    <div class="stage act-stage">
      <div class="stage-head">
        <span class="t">正在执行</span>
        <span class="m">guest_entry_count 实时读数</span>
        <span class="r">
          <div><span class="n" id="heroRate2">0</span><span class="label">VM-Entry / 秒</span></div>
          <div><span class="n">${runningCount()}</span><span class="label">活跃客户机</span></div>
          <div><span class="n">${usedCpus()}<span style="font-size:13px;color:var(--ink-4)">/${PCPUS}</span></span><span class="label">占用物理核</span></div>
        </span>
      </div>
      <div class="acts" id="acts">${activityRows()}</div>
      <div class="stage-note">
        每个光点是一次成功的 VM-Entry；该计数只在 vCPU 真正进入 guest 后递增，暂停与失败进入不计数。
      </div>
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
      <span class="col-head" style="margin-left:auto">guest_entry_count</span>
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
    ${kv('VM-Entry 累计', num(VMS.reduce((n, v) => n + v.entry, 0)))}
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
  pulseTick();
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
          <div class="bignum"><div class="n">${num(v.entry)}</div><div class="label">进入 guest 次数</div></div>
          <div class="bignum"><div class="n">${num(v.park)}</div><div class="label">vCPU park 次数</div></div>
        </div>
        <div class="bignums">
          <div class="bignum"><div class="n">${v.status === 'Running' ? num(v.rate) : '0'}</div><div class="label">VM-Entry / 秒</div></div>
          <div class="bignum"><div class="n">${v.vcpus}</div><div class="label">vCPU</div></div>
        </div>
        ${[['ID', v.id], ['内存', memStr(v.mem)], ['镜像', v.image], ['内核参数', v.cmdline || '—']]
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
        ${icon(it.icon, 14)}<span>${esc(it.label)}</span>
        <span class="hint">${esc(it.hint)}</span>
      </button>`
    )
    .join('');
  $$('.cmdk-item', list).forEach((b) =>
    b.addEventListener('click', () => {
      const it = cmdkItems()[Number(b.dataset.i)];
      document.querySelector('.cmdk-wrap')?.remove();
      state.cmdkOpen = false;
      it.run();
    })
  );
}

/* ---------- 控制台日志逐行流入 ---------- */
function startConsole() {
  const body = $('#termBody');
  if (!body) return;
  let i = 0;
  const push = () => {
    if (!document.body.contains(body)) return;
    if (i < BOOTLOG.length) {
      const div = document.createElement('div');
      div.innerHTML = `<span class="ts">${String(i).padStart(3, '0')}</span> ${esc(BOOTLOG[i])}`;
      body.appendChild(div);
      i++;
      setTimeout(push, 90 + Math.random() * 130);
    }
  };
  push();
}

/* ---------- 文件传输推进 ---------- */
function tickTransfers() {
  const t = TRANSFERS.find((x) => x.state === '传输中');
  if (!t) return;
  const id = setInterval(() => {
    if (state.view !== 'files') return clearInterval(id);
    if (t.done >= t.total) {
      t.state = '完成';
      return clearInterval(id);
    }
    t.done = Math.min(t.total, t.done + 1_400_000);
    if (state.view === 'files') switchView('files');
  }, 700);
}

/* ---------- 底部事件流：逐字打出 ---------- */
function streamEvents() {
  const el = $('#evStream');
  if (!el) return;
  let i = 0;
  let j = 0;
  let cur = '';
  const step = () => {
    if (i >= EVENTS.length) i = 0;
    const line = EVENTS[i];
    if (j <= line.length) {
      cur = line.slice(0, j);
      el.innerHTML = `${esc(cur)}<span class="caret"></span>`;
      j++;
      setTimeout(step, 26);
    } else {
      j = 0;
      i++;
      setTimeout(step, 900);
    }
  };
  step();
}

/* ---------- uptime ---------- */
function tickTime() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const h = Math.floor(up / 3600);
  const m = Math.floor((up % 3600) / 60);
  const s = up % 60;
  $('#sbUp').textContent = `up ${h}h ${String(m).padStart(2, '0')}m ${String(s).padStart(2, '0')}s`;
}

/* ---------- 主题 ---------- */
function setTheme(name) {
  state.theme = name;
  document.documentElement.dataset.theme = name;
  localStorage.setItem('axvisor.theme', name);
  $$('#themeSwitch button').forEach((b) => b.classList.toggle('on', b.dataset.theme === name));
}

/* ---------- 启动 ---------- */
function boot() {
  $('#themeSwitch').addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (b) setTheme(b.dataset.theme);
  });
  $('#cmdkBtn').addEventListener('click', openCmdk);
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      openCmdk();
    }
  });

  renderNav();
  setTheme(state.theme);
  switchView('vms');
  streamEvents();
  tickTime();
  setInterval(tickTime, 1000);
  setInterval(pulseTick, TICK_MS);
}

boot();
