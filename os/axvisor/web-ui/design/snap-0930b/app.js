/* AxVisor 控制台 v6
 *
 * 设计立场（看过 E2B / Modal / Fly.io 首屏后修正）：
 *   这三家的首屏都没有实时遥测。主视觉是「一句价值主张 + 一个具体实体的快照」，
 *   指标只以少量、静态、产品级的大数字出现在中后段。
 *   所以本版把 hero 做成主张，把主视觉做成客户机本身（客户买的就是客户机），
 *   VM-Entry 这类内部计数退回抽屉，供排障者查看。
 *
 * CPU 拓扑不再按核拆成独立小图：核数一多必然碎，且客户不关心核拓扑。
 *   主视图只给一条整机余量；按核的密度带收进宿主机页，做成连续色块，
 *   宽度随核数自适应 —— 8 核与 128 核是同一条。
 *
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

/* entry / park 是 guest_entry_count 与 guest_park_count 的实时读数，
 * 只在抽屉里出现；主视图不展示内部计数。 */
const VMS = [
  { id: 'vm-1', name: 'arceos-shell', os: 'ArceOS', status: 'Running', vcpus: 4, mem: 1.0, cpus: [0, 1, 2, 3], entry: 48213, park: 3, rate: 0, image: 'arceos-x86_64.img', cmdline: 'LOG=info' },
  { id: 'vm-2', name: 'linux-guest', os: 'Linux', status: 'Running', vcpus: 2, mem: 2.0, cpus: [4, 5], entry: 31904, park: 1, rate: 0, image: 'linux-x86_64.img', cmdline: 'console=ttyS0' },
  { id: 'vm-3', name: 'rtos-node', os: 'RTOS', status: 'Running', vcpus: 2, mem: 0.5, cpus: [0, 1], entry: 17440, park: 5, rate: 0, image: 'rtos-x86_64.img', cmdline: '' },
  { id: 'vm-4', name: 'net-peer', os: 'Linux', status: 'Stopped', vcpus: 2, mem: 0.5, cpus: [6, 7], entry: 0, park: 0, rate: 0, image: 'net-x86_64.img', cmdline: '' },
  { id: 'vm-5', name: 'blk-bench', os: 'Linux', status: 'Failed', vcpus: 1, mem: 4.0, cpus: [7], entry: 0, park: 0, rate: 0, image: 'bench-x86_64.img', cmdline: '', issue: 'duplicate-id' },
];

const STATE_CN = { Running: '运行中', Paused: '已暂停', Stopped: '已停止', Failed: '失败' };

const VIEWS = [
  { id: 'vms', label: '客户机', icon: 'i-server', count: () => VMS.length },
  { id: 'console', label: '网页控制台', icon: 'i-terminal' },
  { id: 'files', label: '文件传输', icon: 'i-file' },
  { id: 'shell', label: 'Shell', icon: 'i-shell' },
];
const SYS_VIEWS = [{ id: 'host', label: '宿主机', icon: 'i-cpu' }];

/* `GET /api/files` — 一个会话 = 一个被传到客户机文件系统的文件。
 * 字段与后端 SessionView 对齐：directory 是操作者选的目标目录，
 * 字节先落到 `<directory>/.files/<id>`，place 之后才是 `<directory>/<name>`。 */
const TRANSFERS = [
  { id: 'rootfs.ext4', name: 'rootfs.ext4', directory: '/guest/linux', total: 268435456, done: 268435456, state: '已落盘' },
  { id: 'app.bin', name: 'app.bin', directory: '/guest', total: 12582912, done: 7340032, state: '传输中' },
  { id: 'config.toml', name: 'config.toml', directory: '/guest', total: 4096, done: 0, state: '排队' },
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

/* 底部事件流只写控制面能真正观察到的现象 */
const EVENTS = [
  'vm-1 状态变更 → Running',
  'vm-2 vcpu 1 → VM-Entry',
  'vm-3 vcpu 0 parked：suspend wait',
  'registry：配置池 /guest 变更 1 项',
  'vm-2 状态变更 → Running',
  'vm-5 加载被拒：duplicate-id',
  'vm-1 vcpu 3 → VM-Exit (reason 12)',
  'vm-3 状态变更 → Running',
];

/* ---------- 创建客户机的数据来源 ----------
 * 三个来源全是控制面真实存在的读接口，字段与后端 `GET /api/vms/schema`
 * 逐字对齐；file 字段的"已就位"判定用的就是 `GET /api/vms/browse` 列出的文件，
 * 也就是 create 门与传输落盘读的同一个谓词。 */

/* `GET /api/vms/pool` — 只是候选，start 时才真正创建 */
const POOL = [
  { id: 6, name: 'nimbos-zephyr', path: '/guest/nimbos-zephyr.toml', source: '/guest', cpu_num: 2, memory_mb: 512, entry: '0x80200000' },
  { id: 7, name: 'linux-builder', path: '/guest/linux-builder.toml', source: '/guest', cpu_num: 4, memory_mb: 2048, entry: '0x80200000' },
];

/* `GET /api/vms/browse?path=…` — 文件字段只能选这里已经就位的文件 */
const GUEST_FILES = [
  '/guest/linux/linux-qemu',
  '/guest/linux/rootfs-x86_64-alpine.img',
  '/guest/arceos/arceos-x86_64.bin',
  '/guest/nimbos/nimbos-x86_64.bin',
];

/* `GET /api/files/browse?path=…` — 传输的目标目录。
 * 后端 `POST /api/files` 的 `directory` 必须是客户机文件系统里已经存在的目录
 * （不存在就 409 NoSuchDirectory），要新的目录得先 `POST /api/files/dirs`
 * 建一层，所以这里既是浏览结果，也是"放置位置"的候选。 */
const GUEST_DIRS = ['/guest', '/guest/linux', '/guest/arceos', '/guest/nimbos', '/guest/images'];
const FS_ROOT = '/guest';
const parentOf = (dir) => (dir === FS_ROOT ? null : dir.replace(/\/[^/]+$/, '') || FS_ROOT);
const subdirsOf = (dir) =>
  GUEST_DIRS.filter((d) => d !== dir && d.startsWith(dir === FS_ROOT ? FS_ROOT + '/' : dir + '/'))
    .filter((d) => d.slice(dir.length + 1).indexOf('/') === -1)
    .sort();
const dirExists = (dir) => GUEST_DIRS.includes(dir);

/* `GET /api/vms/schema` — 表单字段集是后端模板的投影，不是界面自己列的 */
const SCHEMA = [
  { name: 'id', type: 'integer', required: true, example: 2, desc: '注册表里必须唯一的数字标识' },
  { name: 'name', type: 'string', required: true, example: 'linux-demo', desc: '显示用的名字' },
  { name: 'kernel_path', type: 'file', required: true, example: '/guest/linux/linux-qemu', desc: '内核镜像在客户机文件系统里的路径，必须已就位' },
  { name: 'rootfs_path', type: 'file', required: false, accept: ['.img'], example: '/guest/linux/rootfs-x86_64-alpine.img', desc: '根盘镜像，留空即无盘客户机' },
  { name: 'entry_point', type: 'address', required: true, default: '0x8020_0000', desc: '内核入口的客户机物理地址' },
  { name: 'kernel_load_addr', type: 'address', required: true, default: '0x8020_0000', desc: '内核装载到的客户机物理地址' },
  { name: 'memory_base', type: 'address', required: true, default: '0x8000_0000', desc: '客户机内存的起始地址' },
  { name: 'memory_mb', type: 'integer', required: true, example: 256, desc: '客户机内存大小（MiB）' },
  { name: 'cpu_num', type: 'integer', required: false, default: 1, desc: '给客户机几个 vCPU' },
  { name: 'guest_type', type: 'enum', required: false, default: 'virtualized', options: ['virtualized', 'passthrough'], desc: 'virtualized：设备全模拟；passthrough：直通物理设备' },
  { name: 'cmdline', type: 'string', required: false, default: 'root=/dev/vda ro rootwait console=ttyS0 init=/bin/sh', desc: '传给内核的命令行' },
];

const inPlace = (path) => GUEST_FILES.includes(path.trim());
/* 预填的 id 要同时避开注册表和配置池：池里的 id 一旦被表单占用，
 * create 会与池条目撞车（后端对这个同样是 409）。 */
const nextFreeId = () =>
  Math.max(0, ...VMS.map((v) => Number(v.id.replace('vm-', '')) || 0), ...POOL.map((p) => p.id)) + 1;
const blankValues = () =>
  Object.fromEntries(
    SCHEMA.map((f) => [f.name, f.default === undefined ? '' : String(f.default)])
  );

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

/* ---------- 客户机卡片 ---------- */
function vmCard(v) {
  const live = v.status === 'Running';
  return `<article class="vm-card${live ? ' live' : ''}" data-vm="${v.id}" tabindex="0">
    <div class="card-top">
      <span class="state ${v.status.toLowerCase()}"><span class="dot"></span>${STATE_CN[v.status]}</span>
      <span class="card-os">${esc(v.os)}</span>
    </div>
    <div class="card-name">${esc(v.name)}</div>
    <div class="card-spec">
      <span><b>${v.vcpus}</b> vCPU</span>
      <span class="q">·</span>
      <span><b>${memStr(v.mem)}</b></span>
    </div>
    <div class="card-pin">物理核 ${v.cpus.join(', ')}</div>
    <div class="card-ops">
      <button class="mini" data-act="console">控制台</button>
      <button class="mini" data-act="${live ? 'stop' : 'start'}">${live ? '停止' : '启动'}</button>
    </div>
  </article>`;
}

/* ---------- 主视图 ---------- */
function viewVms() {
  const used = usedCpus();
  const free = PCPUS - used;

  const capacity = `<div class="capacity">
    <div class="cap-head">
      <span class="t">整机余量</span>
      <span class="m">${HOST.cpus} 个物理核</span>
      <span class="cap-right">
        <span class="n">${used}<span class="of">/${PCPUS}</span></span>
        <span class="lb">已划分</span>
        <span class="n">${free}</span>
        <span class="lb">空闲</span>
      </span>
    </div>
    <div class="cap-bar">
      ${Array.from({ length: PCPUS }, (_, i) => {
        const owners = occupancy()[i];
        const cls = owners.length > 1 ? ' shared' : owners.length === 1 ? ' used' : '';
        return `<i class="${cls.trim()}" title="pCPU ${i}${
          owners.length ? '：' + owners.map((v) => v.name).join(' + ') : '：空闲'
        }"></i>`;
      }).join('')}
    </div>
    <div class="cap-note">
      已分配 <span class="mono">${memStr(totalMem())}</span> 内存
      <span class="q">· 宿主总量不由控制面导出，界面不做估算</span>
    </div>
  </div>`;

  return `
  <section class="hero rise" style="--i:0">
    <div class="hero-top">
      <div class="hero-lead">
        <div class="hero-eyebrow">Type-1 裸机虚拟化</div>
        <div class="hero-sub">
          <span>AxVisor <span class="k">${HOST.version}</span></span>
          <span class="sep">·</span>
          <span>${esc(HOST.platform)}</span>
          <span class="sep">·</span>
          <span>${PCPUS} 个物理核</span>
          <span class="sep">·</span>
          <span>超配比 <span class="k">${(totalVcpus() / PCPUS).toFixed(1)}×</span></span>
        </div>
      </div>
      <div class="hero-snap">
        <div class="snap">
          <div class="s-num" data-count="${VMS.length}">0</div>
          <div class="s-lab">客户机</div>
        </div>
        <div class="snap live">
          <div class="s-num" data-count="${runningCount()}">0</div>
          <div class="s-lab">运行中</div>
        </div>
        <div class="snap">
          <div class="s-num" data-count="${totalVcpus()}">0</div>
          <div class="s-lab">vCPU</div>
        </div>
      </div>
    </div>
    ${capacity}
    <div class="pane-notice" id="paneNotice" hidden></div>
  </section>

  <section class="section rise" style="--i:1">
    <div class="section-head">
      <span class="title">客户机</span>
      <span class="count">${VMS.length}</span>
    </div>
    <div class="card-grid">
      ${VMS.map(vmCard).join('')}
      <article class="vm-card new" tabindex="0">
        <div class="new-plus">+</div>
        <div class="new-label">从配置池加载客户机</div>
      </article>
    </div>
  </section>`;
}

/* 操作被拒时的一句话说明，说在动作发生的地方 */
let noticeTimer = null;
function notice(text) {
  const el = $('#paneNotice');
  if (!el) return;
  el.hidden = false;
  el.innerHTML = `<span>${esc(text)}</span><button class="icon-btn" aria-label="关闭">${icon('i-close', 13)}</button>`;
  $('button', el).addEventListener('click', () => (el.hidden = true));
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => (el.hidden = true), 6000);
}

/* ---------- 宿主机：按核密度带（核数自适应） ---------- */
function coreDensity() {
  const occ = occupancy();
  return `<div class="density" style="--n:${PCPUS}">
    ${occ
      .map((owners, i) => {
        const cls = owners.length > 1 ? 'shared' : owners.length === 1 ? 'used' : '';
        return `<i class="${cls}" title="pCPU ${i}${
          owners.length ? '：' + owners.map((v) => v.name).join(' + ') : '：空闲'
        }"></i>`;
      })
      .join('')}
  </div>`;
}

function viewHost() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const h = Math.floor(up / 3600);
  const m = Math.floor((up % 3600) / 60);
  const kv = (k, v) => `<div class="kv"><span class="k">${k}</span><span class="v">${v}</span></div>`;
  const occ = occupancy();
  const shared = occ.filter((o) => o.length > 1).length;
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

    <div class="host-block">
      <div class="block-head">
        <span class="t">物理核占用</span>
        <span class="m">${usedCpus()} 已划分 · ${shared} 个被分时共享</span>
      </div>
      ${coreDensity()}
      <div class="block-note">
        每个色块是一个物理核；被多台客户机共用的核标为共享。核数增加时色块自动变窄，形态不变。
      </div>
    </div>
  </div>`;
}

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
  const rows = TRANSFERS.map((t) => {
    const pct = ((t.done / t.total) * 100).toFixed(1);
    const live = t.state === '传输中' || t.state === '排队';
    const target = t.path || `${t.directory}/${t.name}`;
    return `<div class="xfer${live ? ' active' : ''}${t.state === '已取消' ? ' dead' : ''}">
      <div class="x-main">
        <div class="x-title">${esc(t.name)}<span class="q"> → ${esc(target)}</span></div>
        <div class="x-dir mono">目标目录 ${esc(t.directory)}</div>
        <div class="bar"><i style="width:${pct}%"></i></div>
      </div>
      <span class="st">${t.state}</span>
      <span class="pct">${bytes(t.done)} / ${bytes(t.total)}</span>
      <div class="x-ops">
        ${live ? `<button class="mini" data-cancel="${esc(t.id)}">取消</button>` : ''}
        ${t.state === '已取消' ? `<button class="mini go" data-resume="${esc(t.id)}">续传</button>` : ''}
      </div>
    </div>`;
  }).join('');

  return `<div class="pane">
    <h2>文件传输</h2>
    <div class="sub">
      本机文件传到客户机文件系统之后，才能被客户机配置的字段引用（内核镜像、根盘镜像）。
      一个会话是 <span class="mono">POST /api/files</span> 打开的，分片走
      <span class="mono">PATCH /api/files/{id}</span>（<span class="mono">Content-Range</span> 给出区间），
      中断后用 <span class="mono">HEAD /api/files/{id}</span> 取回偏移继续，
      收完 <span class="mono">POST /api/files/{id}/place</span> 落为最终名字。
    </div>
    ${rows || '<div class="xfer empty"><span class="q">还没有会话——在新建客户机的表单里用「本机…」选一个文件。</span></div>'}
  </div>`;
}

function cancelTransfer(id) {
  const t = TRANSFERS.find((x) => x.id === id);
  if (!t) return;
  t.state = '已取消';
  pushEvent(`取消传输 ${t.name} —— DELETE /api/files/{id} 丢弃会话与暂存字节`);
  switchView('files');
}

function resumeTransfer(id) {
  const t = TRANSFERS.find((x) => x.id === id);
  if (!t) return;
  t.state = '传输中';
  pushEvent(`续传 ${t.name}：从 ${bytes(t.done)} 处继续（HEAD /api/files/{id}）`);
  switchView('files');
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

const RENDER = { vms: viewVms, console: viewConsole, files: viewFiles, shell: viewShell, host: viewHost };

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
  $$('.vm-card[data-vm]').forEach((card) => {
    const id = card.dataset.vm;
    card.addEventListener('click', (e) => {
      if (e.target.closest('[data-act]')) return;
      openDrawer(id);
    });
    card.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') openDrawer(id);
    });
  });
  $$('.vm-card.new').forEach((card) => {
    card.addEventListener('click', () => openCreateSheet());
    card.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') openCreateSheet();
    });
  });
  $$('[data-act="console"]').forEach((b) =>
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      switchView('console');
    })
  );
  $$('[data-cancel]').forEach((b) => b.addEventListener('click', () => cancelTransfer(b.dataset.cancel)));
  $$('[data-resume]').forEach((b) => b.addEventListener('click', () => resumeTransfer(b.dataset.resume)));
  /* start / stop 走控制面的真实语义：stop 后重启是被拒的（409），
   * 这不是界面偷懒，是后端当前的限制。 */
  $$('[data-act="start"], [data-act="stop"]').forEach((b) =>
    b.addEventListener('click', (e) => {
      e.stopPropagation();
      const v = VMS.find((x) => x.id === b.closest('.vm-card').dataset.vm);
      if (!v) return;
      if (b.dataset.act === 'start' && v.status === 'Stopped') {
        pushEvent(`vm-${v.id.replace('vm-', '')} 启动被拒：409 —— 停止后的客户机不支持重启`);
        notice(`「${v.name}」已停止，控制面不支持停止后重启（409）。删除后可从配置池重建。`);
        return;
      }
      if (b.dataset.act === 'start') {
        v.status = 'Running';
        v.cpus = pickCpus(v.vcpus);
        pushEvent(`vm-${v.id.replace('vm-', '')} 已启动：${v.name} → 物理核 ${v.cpus.join(', ')}`);
      } else {
        v.status = 'Stopped';
        v.cpus = [];
        v.rate = 0;
        pushEvent(`vm-${v.id.replace('vm-', '')} 停止请求已接受（异步完成）：${v.name}`);
      }
      switchView('vms');
    })
  );
  if (state.view === 'console') startConsole();
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

/* 内部计数仍在前推，但只在抽屉里被看到 */
function pulseTick() {
  VMS.forEach((v) => {
    if (v.status !== 'Running') {
      v.rate = 0;
      return;
    }
    const step = 18 + Math.round(Math.random() * 46);
    v.entry += step;
    v.rate = Math.round(step * (1000 / TICK_MS));
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
          <div class="bignum"><div class="n">${v.vcpus}</div><div class="label">vCPU</div></div>
          <div class="bignum"><div class="n">${memStr(v.mem)}</div><div class="label">内存</div></div>
        </div>
        ${[['ID', v.id], ['系统', v.os], ['镜像', v.image], ['内核参数', v.cmdline || '—']]
          .map(([k, val]) => `<div class="field"><span class="k">${k}</span><span class="v">${esc(val)}</span></div>`)
          .join('')}
        <div class="field">
          <span class="k">物理核亲和</span>
          <span class="v"><span class="pin-grid">${Array.from(
            { length: PCPUS },
            (_, i) => `<span class="pin${v.cpus.includes(i) ? ' on' : ''}">${i}</span>`
          ).join('')}</span></span>
        </div>
        <div class="field">
          <span class="k">进入 guest 次数</span>
          <span class="v mono">${num(v.entry)}${v.status === 'Running' ? `（${num(v.rate)}/s）` : ''}</span>
        </div>
        <div class="field">
          <span class="k">vCPU park 次数</span>
          <span class="v mono">${num(v.park)}</span>
        </div>
        ${v.issue ? `<div class="notice">配置问题：<span class="mono">${esc(v.issue)}</span> — 该客户机的 ID 与池中另一项重复，未加载。</div>` : ''}
      </div>
    </aside>`;
  document.body.appendChild(wrap);
  const scrim = $('.scrim', wrap);
  const drawer = $('.drawer', wrap);
  void drawer.offsetWidth;
  scrim.classList.add('in');
  drawer.classList.add('in');
  $$('[data-close]', wrap).forEach((el) => el.addEventListener('click', () => closeDrawer(wrap)));
}

function closeDrawer(wrap) {
  if (!wrap) return;
  $('.scrim', wrap)?.classList.remove('in');
  $('.drawer', wrap)?.classList.remove('in');
  setTimeout(() => wrap.remove(), 420);
}

/* ---------- 新建客户机 ---------- */
let createState = null;

/* 物理核：优先挑没人用的，不够就从占得最少的核上叠（超配比本来就是允许的） */
function pickCpus(n) {
  const occ = occupancy();
  const free = occ.map((o, i) => [i, o.length]).filter(([, c]) => c === 0).map(([i]) => i);
  if (free.length >= n) return free.slice(0, n);
  const byLoad = occ
    .map((o, i) => [i, o.length])
    .sort((a, b) => a[1] - b[1])
    .map(([i]) => i);
  return [...free, ...byLoad.filter((i) => !free.includes(i))].slice(0, n);
}

const guessOs = (kernelPath) =>
  /arceos/i.test(kernelPath) ? 'ArceOS'
  : /nimbos/i.test(kernelPath) ? 'NimbOS'
  : /linux/i.test(kernelPath) ? 'Linux'
  : 'Guest';

function openCreateSheet(prefill) {
  if ($('.drawer.create')) return;
  createState = {
    values: Object.assign(blankValues(), { id: String(nextFreeId()) }, prefill || {}),
    error: '',
    /* 本机选中的文件，按字段存；传给客户机文件系统之前它不能作为字段值提交 */
    local: {},
    target: {},
    xfer: {},
  };
  const wrap = document.createElement('div');
  wrap.innerHTML = `<div class="scrim" data-close></div>
    <aside class="drawer wide create" role="dialog" aria-label="新建客户机">
      <div class="drawer-head">
        <div style="flex:1">
          <div class="t">新建客户机</div>
          <div class="s">字段集来自 <span class="mono">GET /api/vms/schema</span>，提交为 <span class="mono">POST /api/vms/create</span></div>
        </div>
        <button class="icon-btn" data-close aria-label="关闭">${icon('i-close', 15)}</button>
      </div>
      <div class="drawer-body" id="createBody"></div>
      <div class="sheet-foot" id="createFoot"></div>
    </aside>`;
  document.body.appendChild(wrap);
  /* 强制一次 reflow 让初始样式先落定，再入 .in 播过渡——
   * 不依赖 rAF，无头浏览器与降速环境下同样可靠。 */
  const scrim = $('.scrim', wrap);
  const drawer = $('.drawer', wrap);
  void drawer.offsetWidth;
  scrim.classList.add('in');
  drawer.classList.add('in');
  $$('[data-close]', wrap).forEach((el) => el.addEventListener('click', () => closeDrawer(wrap)));

  $('#createBody', wrap).addEventListener('input', (e) => {
    const name = e.target.dataset.f;
    if (!name) return;
    createState.values[name] = e.target.value;
    createState.error = '';
    renderCreateFoot();
    renderCreateWarnings();
  });
  $('#createBody', wrap).addEventListener('change', (e) => {
    const fileField = e.target.dataset.file;
    if (fileField && e.target.files && e.target.files[0]) {
      return onLocalPicked(fileField, e.target.files[0]);
    }
    const name = e.target.dataset.f;
    if (!name) return;
    createState.values[name] = e.target.value;
    renderCreateFoot();
    renderCreateWarnings();
  });
  $('#createBody', wrap).addEventListener('click', (e) => {
    const pick = e.target.closest('[data-pick]');
    if (pick) return openPicker(pick.dataset.pick);
    const local = e.target.closest('[data-local]');
    if (local) return pickLocal(local.dataset.local);
    const xfer = e.target.closest('[data-xfer]');
    if (xfer) return startTransfer(xfer.dataset.xfer);
    const poolStart = e.target.closest('[data-pool]');
    if (poolStart) return startFromPool(Number(poolStart.dataset.pool), wrap);
    const dt = e.target.closest('[data-dir]');
    if (dt) return openDirPicker(dt.dataset.dir);
  });

  renderCreate();
}

function renderCreate() {
  const body = $('#createBody');
  if (!body) return;
  const values = createState.values;

  const poolRows = POOL.map(
    (p) => `<div class="pool-row">
      <div class="pool-main">
        <div class="pool-name">${esc(p.name)}<span class="pool-id mono">ID ${p.id}</span></div>
        <div class="pool-meta">${esc(p.path)} · 来源 ${esc(p.source)} · ${p.cpu_num} vCPU · ${p.memory_mb} MiB</div>
      </div>
      <button class="mini go" data-pool="${p.id}">启动</button>
    </div>`
  ).join('');

  const form = SCHEMA.map((f) => {
    const v = values[f.name] ?? '';
    const tag = f.required ? '<span class="req">必填</span>' : '<span class="opt">可选</span>';
    const ph = f.example === undefined ? '' : ` placeholder="${esc(String(f.example))}"`;
    let control;
    if (f.type === 'enum') {
      control = `<select class="f-in" data-f="${f.name}">${f.options
        .map((o) => `<option${o === v ? ' selected' : ''}>${o}</option>`)
        .join('')}</select>`;
    } else if (f.type === 'file') {
      control = `<div class="f-file">
        <input class="f-in" data-f="${f.name}" value="${esc(v)}"${ph} />
        <button class="mini" data-pick="${f.name}">已就位…</button>
        <button class="mini" data-local="${f.name}">本机…</button>
      </div>
      <input type="file" data-file="${f.name}" hidden />`;
    } else {
      control = `<input class="f-in" data-f="${f.name}" value="${esc(v)}"${ph} />`;
    }
    return `<div class="ff" data-row="${f.name}">
      <div class="ff-k">${f.name}<span class="ff-t mono">${f.type}</span>${tag}</div>
      <div class="ff-v">${control}<div class="ff-d">${esc(f.desc)}</div><div class="ff-w"></div></div>
    </div>`;
  }).join('');

  body.innerHTML = `
    <div class="block">
      <div class="block-head"><span class="t">从配置池启动</span><span class="m">GET /api/vms/pool · 条目自带完整配置，start 时按它创建，不需要填表</span></div>
      ${poolRows}
    </div>
    <div class="block">
      <div class="block-head"><span class="t">填表创建</span><span class="m">字段随后端模板变化，界面不复制一份字段表</span></div>
      ${form}
      <div class="form-note">
        内核来源只有 <span class="mono">fs</span>：填表创建的客户机从客户机文件系统读内核，
        所以 <span class="mono">kernel_path</span> 必须是已就位的文件。
      </div>
    </div>`;

  renderCreateWarnings();
  renderCreateFoot();
}

/* 每个 file 字段的即时提示。
 * 字段只接受客户机文件系统里的路径（/ 开头）；本机文件是另一个命名空间，
 * 必须先经「文件传输」落到客户机文件系统，这正是 create 门读的谓词。 */
function renderCreateWarnings() {
  const values = createState.values;
  SCHEMA.filter((f) => f.type === 'file').forEach((f) => {
    const row = $(`[data-row="${f.name}"] .ff-w`);
    if (!row) return;
    const path = (values[f.name] ?? '').trim();
    const local = createState.local[f.name];
    const item = createState.xfer[f.name];
    let text = '';

    if (item) {
      const pct = ((item.done / item.total) * 100).toFixed(0);
      text = `<span class="ok">正在传输 ${esc(item.name)} → ${esc(item.path)} · ${pct}%</span>`;
    } else if (local) {
      const dir = createState.target[f.name] || FS_ROOT;
      text = `<span class="warn">本机文件 ${esc(local.name)}（${bytes(local.size)}），不在客户机文件系统中</span>
        <span class="dirline">放到 <button class="mini" data-dir="${f.name}">${esc(dir)} ▾</button>
        <span class="q">落盘为 ${esc(dir)}/${esc(local.name)}</span></span>
        <button class="mini go" data-xfer="${f.name}">传入并填入</button>`;
    } else if (path && !path.startsWith('/')) {
      text = `<span class="warn">这是本机路径；字段只接受客户机文件系统里的路径</span>
        <button class="mini go" data-local="${f.name}">从本机选文件</button>`;
    } else if (path && !inPlace(path)) {
      text = `<span class="warn">不在客户机文件系统中 —— 提交会被 409 拒，并提示先传输</span>
        <button class="mini go" data-local="${f.name}">从本机传入</button>`;
    } else if (path && f.accept && !f.accept.some((ext) => path.toLowerCase().endsWith(ext))) {
      text = `<span class="warn">该字段只接受 ${f.accept.join(' / ')}</span>`;
    } else if (path) {
      text = `<span class="ok">已就位 · ${esc(path)}</span>`;
    }
    row.innerHTML = text;
  });
}

/* 本机文件只能先传到客户机文件系统：路径落在 /guest 下，传完自动填入字段 */
function pickLocal(field) {
  const input = $(`[data-file="${field}"]`);
  if (input) input.click();
}

function onLocalPicked(field, file) {
  createState.local[field] = { name: file.name, size: file.size || 1024 * 1024 };
  /* 默认落进客户机文件系统的根目录；目录由操作者改，不是界面替他决定 */
  createState.target[field] = createState.target[field] || FS_ROOT;
  createState.error = '';
  renderCreateWarnings();
  renderCreateFoot();
}

/* 目标目录选择器：`POST /api/files` 的 `directory` 必须是已存在的目录，
 * 所以这里只列已经存在的目录，新建走 `POST /api/files/dirs`（一次一层）。 */
function openDirPicker(field) {
  let cursor = createState.target[field] || FS_ROOT;
  const wrap = document.createElement('div');
  wrap.className = 'picker-wrap';
  const paint = () => {
    const subs = subdirsOf(cursor);
    wrap.innerHTML = `<div class="picker" role="dialog" aria-label="选择放置目录">
      <div class="picker-head">${esc(cursor)}<span class="m">GET /api/files/browse</span></div>
      <div class="pick-row static">
        <span class="q">目标目录必须已经存在；字节先落到 <span class="mono">${esc(cursor)}/.files/&lt;id&gt;</span>，落盘后是 <span class="mono">${esc(cursor)}/&lt;文件名&gt;</span></span>
      </div>
      <div class="pick-ops">
        <button class="mini" data-up>上级</button>
        <button class="mini" data-mk>新建文件夹</button>
        <button class="mini go" data-ok>选定此目录</button>
      </div>
      ${subs.length
        ? subs.map((d) => `<button class="pick-row" data-cd="${esc(d)}"><span class="mono">[dir] ${esc(d)}</span></button>`).join('')
        : '<div class="pick-row static"><span class="q">这个目录下没有子目录</span></div>'}
    </div>`;
  };
  paint();
  document.body.appendChild(wrap);
  const close = () => wrap.remove();
  wrap.addEventListener('click', (e) => {
    if (e.target === wrap) return close();
    const cd = e.target.closest('[data-cd]');
    if (cd) {
      cursor = cd.dataset.cd;
      return paint();
    }
    if (e.target.closest('[data-up]')) {
      const up = parentOf(cursor);
      if (up) cursor = up;
      return paint();
    }
    if (e.target.closest('[data-mk]')) {
      const name = window.prompt('新建一层文件夹（后端一次只建一层）', 'images');
      if (!name) return;
      const made = `${cursor}/${name.replace(/^\/+|\/+$/g, '')}`;
      if (!dirExists(made)) GUEST_DIRS.push(made);
      cursor = made;
      pushEvent(`新建目录 ${made}`);
      return paint();
    }
    if (e.target.closest('[data-ok]')) {
      createState.target[field] = cursor;
      renderCreateWarnings();
      renderCreateFoot();
      return close();
    }
  });
}

function startTransfer(field) {
  const local = createState.local[field];
  if (!local) return;
  const dir = createState.target[field] || FS_ROOT;
  const item = {
    id: local.name,
    name: local.name,
    directory: dir,
    path: `${dir}/${local.name}`,
    total: local.size,
    done: 0,
    state: '传输中',
  };
  createState.xfer[field] = item;
  TRANSFERS.push(item);
  pushEvent(`开始传输 ${local.name} → ${item.path}`);
  renderCreateWarnings();
}

/* 传输推进：POST /api/files 的分片在这里只是进度，落盘后路径才进客户机文件系统 */
function transferTick() {
  let moved = false;
  TRANSFERS.forEach((t) => {
    if (t.state === '排队') {
      t.state = '传输中';
      moved = true;
    }
    if (t.state !== '传输中') return;
    t.done = Math.min(t.total, t.done + Math.max(t.total * 0.18, 1));
    moved = true;
    if (t.done >= t.total) {
      t.state = '已落盘';
      if (!GUEST_FILES.includes(t.path)) GUEST_FILES.push(t.path);
      pushEvent(`传输完成：${t.path}`);
    }
  });
  if (!createState) {
    if (moved && state.view === 'files') switchView('files');
    return;
  }
  Object.keys(createState.xfer).forEach((field) => {
    const item = createState.xfer[field];
    if (item.state !== '已落盘') return;
    createState.values[field] = item.path;
    delete createState.local[field];
    delete createState.target[field];
    delete createState.xfer[field];
    const input = $(`[data-f="${field}"]`);
    if (input) input.value = item.path;
  });
  renderCreateWarnings();
  renderCreateFoot();
  if (moved && state.view === 'files') switchView('files');
}

function renderCreateFoot() {
  const foot = $('#createFoot');
  if (!foot) return;
  const values = createState.values;
  const missing = SCHEMA.filter((f) => f.type === 'file')
    .map((f) => (values[f.name] ?? '').trim())
    .filter((p) => p && !inPlace(p));

  foot.innerHTML = `
    ${createState.error ? `<div class="foot-err">${esc(createState.error)}</div>` : ''}
    ${missing.length && !createState.error
      ? `<div class="foot-warn">未就位：${missing.map(esc).join('、')}</div>`
      : ''}
    <div class="foot-ops">
      <button class="btn" id="createOnly">仅创建</button>
      <button class="btn btn-primary" id="createStart">创建并启动</button>
    </div>`;

  $('#createOnly', foot)?.addEventListener('click', () => submitCreate(false));
  $('#createStart', foot)?.addEventListener('click', () => submitCreate(true));
}

/* 选已就位文件：列表来自 GET /api/vms/browse */
function openPicker(fieldName) {
  const wrap = document.createElement('div');
  wrap.className = 'picker-wrap';
  wrap.innerHTML = `<div class="picker" role="dialog" aria-label="选择已就位文件">
      <div class="picker-head">/guest<span class="m">GET /api/vms/browse</span></div>
      ${GUEST_FILES.map(
        (p) => `<button class="pick-row" data-path="${esc(p)}">
          <span class="mono">${esc(p)}</span>
          <span class="st">在客户机文件系统中</span>
        </button>`
      ).join('')}
    </div>`;
  document.body.appendChild(wrap);
  const close = () => wrap.remove();
  wrap.addEventListener('click', (e) => {
    const row = e.target.closest('[data-path]');
    if (row) {
      createState.values[fieldName] = row.dataset.path;
      createState.error = '';
      const input = $(`[data-f="${fieldName}"]`);
      if (input) input.value = row.dataset.path;
      renderCreateWarnings();
      renderCreateFoot();
      close();
      return;
    }
    if (e.target === wrap) close();
  });
}

function submitCreate(andStart) {
  const values = createState.values;
  for (const f of SCHEMA) {
    const text = (values[f.name] ?? '').trim();
    if (text.length === 0) {
      if (f.required) return fail(`「${f.name}」是必填项`);
      continue;
    }
    if (f.type === 'integer' && !/^\d+$/.test(text)) {
      return fail(`「${f.name}」要一个非负整数，收到「${text}」`);
    }
    if (f.type === 'address' && !/^(0[xX][0-9a-fA-F_]+|\d+)$/.test(text.replace(/_/g, ''))) {
      return fail(`「${f.name}」要一个地址（十进制或 0x 开头），收到「${text}」`);
    }
    if (f.type === 'enum' && f.options && !f.options.includes(text)) {
      return fail(`「${f.name}」只能是 ${f.options.join(' / ')}`);
    }
  }

  const id = Number(values.id);
  if (VMS.some((v) => Number(v.id.replace('vm-', '')) === id) || POOL.some((p) => p.id === id)) {
    return fail(`ID ${id} 已被注册表或配置池占用 —— 后端返回 409 Conflict`);
  }
  /* 本机路径不是客户机文件系统里的路径：直接提交等于让 create 去读一个它
   * 根本没有的文件，这里先说清楚，而不是等 409。 */
  const localPath = SCHEMA.filter((f) => f.type === 'file')
    .map((f) => (values[f.name] ?? '').trim())
    .find((p) => p && !p.startsWith('/'));
  if (localPath) {
    return fail(`「${localPath}」是本机路径，字段只接受客户机文件系统里的路径；先用「本机…」把它传进去`);
  }
  const unplaced = SCHEMA.filter((f) => f.type === 'file')
    .map((f) => (values[f.name] ?? '').trim())
    .filter((p) => p && !inPlace(p));
  if (unplaced.length) {
    return fail(`「${unplaced[0]}」不在客户机文件系统中，先传输 —— 后端返回 409 并提示先传输`);
  }

  const vcpus = Number(values.cpu_num || 1);
  const vm = {
    id: `vm-${id}`,
    name: values.name,
    os: guessOs(values.kernel_path),
    status: andStart ? 'Running' : 'Stopped',
    vcpus,
    mem: Number(values.memory_mb) / 1024,
    cpus: andStart ? pickCpus(vcpus) : [],
    entry: 0,
    park: 0,
    rate: 0,
    image: values.kernel_path.split('/').pop(),
    cmdline: values.cmdline,
    config: `${values.name}.toml`,
  };
  VMS.push(vm);
  pushEvent(`vm-${id} 创建${andStart ? '并启动' : ''}成功：${vm.name}（${vcpus} vCPU · ${values.memory_mb} MiB）`);
  closeDrawer($('.drawer.create')?.parentElement);
  createState = null;
  switchView('vms');

  function fail(message) {
    createState.error = message;
    renderCreateFoot();
    const err = $('.foot-err');
    if (err) {
      err.animate(
        [
          { transform: 'translateX(0)' },
          { transform: 'translateX(-4px)' },
          { transform: 'translateX(4px)' },
          { transform: 'translateX(0)' },
        ],
        { duration: 180, easing: 'ease-out' }
      );
    }
  }
}

/* POST /api/vms/{id}/start：未注册的 id 由控制面先从池条目创建，所以池里的一项
 * 可以直接启动，不需要先 create。 */
function startFromPool(poolId, wrap) {
  const entry = POOL.find((p) => p.id === poolId);
  if (!entry) return;
  const existing = VMS.find((v) => Number(v.id.replace('vm-', '')) === entry.id);
  if (existing) {
    if (existing.status === 'Stopped') {
      pushEvent(`vm-${entry.id} 启动被拒：409 —— 停止后的客户机不支持重启`);
    } else {
      existing.status = 'Running';
      existing.cpus = pickCpus(existing.vcpus);
      pushEvent(`vm-${entry.id} 已启动：${existing.name}`);
    }
  } else {
    VMS.push({
      id: `vm-${entry.id}`,
      name: entry.name,
      os: guessOs(entry.name),
      status: 'Running',
      vcpus: entry.cpu_num,
      mem: entry.memory_mb / 1024,
      cpus: pickCpus(entry.cpu_num),
      entry: 0,
      park: 0,
      rate: 0,
      image: `${entry.name}.img`,
      cmdline: '',
      config: entry.path.split('/').pop(),
    });
    pushEvent(`vm-${entry.id} 由配置池创建并启动：${entry.name}（${entry.path}）`);
  }
  closeDrawer(wrap);
  createState = null;
  switchView('vms');
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
  const actions = [
    {
      icon: 'i-plus',
      label: '新建客户机',
      hint: '字段来自 schema',
      run: () => {
        switchView('vms');
        setTimeout(() => openCreateSheet(), 60);
      },
    },
    ...POOL.map((p) => ({
      icon: 'i-play',
      label: `启动 ${p.name}`,
      hint: '配置池',
      run: () => {
        switchView('vms');
        setTimeout(() => startFromPool(p.id, null), 60);
      },
    })),
  ];
  return [...actions, ...views, ...vms].filter((i) => !q || i.label.toLowerCase().includes(q));
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

/* ---------- 控制台日志 ---------- */
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

/* ---------- 底部事件流 ---------- */
const EVENT_QUEUE = [];
const pushEvent = (text) => EVENT_QUEUE.push(text);

function streamEvents() {
  const el = $('#evStream');
  if (!el) return;
  let i = 0;
  let j = 0;
  let line = EVENTS[0];
  const step = () => {
    if (j === 0) {
      line = EVENT_QUEUE.length ? EVENT_QUEUE.shift() : EVENTS[i];
    }
    if (j <= line.length) {
      el.innerHTML = `${esc(line.slice(0, j))}<span class="caret"></span>`;
      j++;
      setTimeout(step, 26);
    } else {
      j = 0;
      i = (i + 1) % EVENTS.length;
      setTimeout(step, 900);
    }
  };
  step();
}

function tickTime() {
  const up = Math.floor((Date.now() - BOOT) / 1000);
  const h = Math.floor(up / 3600);
  const m = Math.floor((up % 3600) / 60);
  const s = up % 60;
  $('#sbUp').textContent = `up ${h}h ${String(m).padStart(2, '0')}m ${String(s).padStart(2, '0')}s`;
}

function setTheme(name) {
  state.theme = name;
  document.documentElement.dataset.theme = name;
  localStorage.setItem('axvisor.theme', name);
  $$('#themeSwitch button').forEach((b) => b.classList.toggle('on', b.dataset.theme === name));
}

function boot() {
  $('#themeSwitch').addEventListener('click', (e) => {
    const b = e.target.closest('button');
    if (b) setTheme(b.dataset.theme);
  });
  $('#cmdkBtn').addEventListener('click', openCmdk);
  $('#createBtn').addEventListener('click', () => openCreateSheet());
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
  setInterval(transferTick, 600);
}

boot();
