# AxVisor 管理 HTTP 控制面 — QEMU hostfwd + curl 操作指南

> 管理 API 的单元/自测（`http-test` feature + tower `oneshot`）不经过真实 TCP；
> 本文档描述用 QEMU hostfwd 端口转发走**真实网络栈**验证的完整流程，并声明该
> 接口的安全边界。**所有命令在工作区根目录（`tgoskits/`）执行**——构建产物位于
> 工作区 `target/`，不是 `os/axvisor/target/`。

---

## 1. 架构

```
host: curl http://localhost:18081/...
        │  TCP
        ▼
QEMU user-mode NAT (hostfwd=tcp::18081-:8080)
        │  转发到虚拟机内 8080
        ▼
AxVisor (hypervisor, EL2) — 管理 HTTP 服务器 (axum + tokio, 可配绑定)
        │  同步调用 AxvmManager API
        ▼
vCPU (Core 1+) — guest
```

- **HTTP 服务器运行在 AxVisor（host）上，不在 guest 里。** 网络输入直接进入 hypervisor。
- **监听地址可配置（默认 loopback）：** 服务器默认绑 `127.0.0.1:8080`；本文档的 hostfwd
  流程需要监听所有接口，故构建 config 显式设置 `[env] AXVM_HTTP_BIND = "0.0.0.0:8080"`。
- QEMU user 模式网络在内部做 NAT：host 的 18081 端口 → 虚拟机的 8080 端口。
- **hostfwd 仅 QEMU user 模式 netdev 可用**（`-netdev user,id=net0,hostfwd=...`），
  不能用 tap/bridge。user 模式性能差，只适合开发/调试。

---

## 2. aarch64 快速开始（推荐，TCG 无需 KVM）

控制 API 验证需要 guest 真实 boot，而 `http-test` 内置自测会把 VM stop（Core 1 idle
触发 restart-after-stop 调度限制）。所以手工验证用**手动 config**（`features =
["no-auto-start", "http-axum"]`，**不含 http-test**）构建，默认 VM 保持 `Ready`，由 curl
驱动启停。config 位于 `os/axvisor/tmp/configs/http-control-manual.toml`（本地 gitignore，
内容如下，可自行重建）：

```toml
features = ["no-auto-start", "http-axum"]
log = "Info"
target = "aarch64-unknown-none-softfloat"
vm_configs = ["test-suit/axvisor/normal/qemu-http-axum-control/aarch64-arceos-http-control.toml"]

# hostfwd 需要服务器监听所有接口；写路由用构建期 token 保护（见 §5）
[env]
AXVM_HTTP_TOKEN = "axvisor-http-test-token"
AXVM_HTTP_BIND = "0.0.0.0:8080"
```

### 2.1 构建

```bash
# 工作区根目录执行

# 1. 预置 guest 内核：控制测试 boot 的 arceos-qemu 来自 registry 的 qemu-aarch64 包，
#    pull 到 tmp/axbuild/images/（与 CI 的 http-axum-control / timer-stress 步骤一致；
#    该文件在测试/构建时被 build.rs include_bytes! 嵌入 hypervisor 镜像）
cargo xtask image pull qemu-aarch64 --output-dir tmp/axbuild/images

# 2. 构建（手动 config，无 http-test，默认 VM 保持 Ready）
cargo xtask axvisor build --config os/axvisor/tmp/configs/http-control-manual.toml
```

> **to_bin 陷阱：** `build`（无 QEMU case）**不会重新生成 `.bin`**——to_bin 由 QEMU
> case config 决定。QEMU 会引导到旧 test 产物 `.bin`（带 `http-test`）。手工从 ELF
> 重新生成：
>
> ```bash
> llvm-objcopy --strip-all -O binary \
>   target/aarch64-unknown-linux-musl/release/axvisor \
>   target/aarch64-unknown-linux-musl/release/axvisor.bin
> ```

### 2.2 引导 QEMU（background）+ hostfwd

```bash
qemu-system-aarch64 -nographic -cpu cortex-a72 \
  -machine virt,virtualization=on,gic-version=3 -smp 2 -m 1g \
  -kernel target/aarch64-unknown-linux-musl/release/axvisor.bin \
  -netdev user,id=net0,hostfwd=tcp::18081-:8080 \
  -device virtio-net-pci,netdev=net0
```

引导日志关键标记：`Initialize network subsystem...`、`use NIC 0: "virtio-net"`（真实网络
枚举）、`management HTTP server (axum) listening on 0.0.0.0:8080`（前缀 `0:12` = Core 0
task 12，核隔离：管理面在 Core 0）、`shell task on CPU0`。vCPU 启动后日志有
`VM[1] VCpu[0] running on CPU1`（vCPU 在 Core 1）。

### 2.3 curl 全流程

```bash
# 所有写命令（POST/DELETE）都需要 Bearer token（与 config 的 AXVM_HTTP_TOKEN 一致）。
TOKEN='Authorization: Bearer axvisor-http-test-token'

# 只读（开放，无需 token）：VM 保持 Ready
curl -s http://localhost:18081/api/vms           # 200, JSON 数组（status="ready"）
curl -s http://localhost:18081/api/vms/1         # 200, 明细（含 vcpu_states）
curl -s http://localhost:18081/api/vms/999       # 404

# 无 token 的写请求被拒绝（401）：认证边界回归
curl -s -i -X POST http://localhost:18081/api/vms/1/start  # 401 Unauthorized

# 控制：start -> guest 运行 -> guest 退出 -> stopped
curl -s -X POST -H "$TOKEN" http://localhost:18081/api/vms/1/start   # 200 {"ok":true,"status":"running","async":false}
curl -s http://localhost:18081/api/vms/1                # 200, status="stopped"（guest 退出后）
curl -s -X POST -H "$TOKEN" http://localhost:18081/api/vms/999/start # 404
curl -s -X POST -H "$TOKEN" http://localhost:18081/api/vms/1/stop    # 200 {"ok":true,"status":"stopping","async":true}（对已停 VM 也 200，幂等）
curl -s -X POST -H "$TOKEN" http://localhost:18081/api/vms/1/start   # 409（restart-after-stop 不支持，契约拒绝）

# 动态创建/删除：create -> ready -> 409（重复 id）-> remove -> 404
curl -s -X DELETE -H "$TOKEN" http://localhost:18081/api/vms/1       # 204（先删除默认 VM，释放其 id）
curl -s -X POST http://localhost:18081/api/vms/create \
  -H "$TOKEN" -H 'Content-Type: application/json' \
  -d '{"toml": "<完整 TOML 配置，base.id 必须命中已内嵌镜像且未注册>"}'  # 200 {"id":1}
curl -s http://localhost:18081/api/vms/1                # 200, status="ready"（重建后）
curl -s -X POST http://localhost:18081/api/vms/create \
  -H "$TOKEN" -H 'Content-Type: application/json' \
  -d '{"toml": "<同上>"}'                                # 409（id 已注册，重复 create 是契约错误）
curl -s -X DELETE -H "$TOKEN" http://localhost:18081/api/vms/1       # 204
curl -s http://localhost:18081/api/vms/1                # 404（已移除）
```

> **create 的镜像约束：** guest 内核只能在构建期内嵌（`image_location = "memory"` →
> `build.rs` include_bytes!），运行时按 `base.id` 严格匹配内嵌镜像
> （`memory_images_for_vm`，boot/images/mod.rs:223-238）。因此 create 只能复现**构建期
> 已内嵌且当前未注册**的 id（即先 DELETE 释放 id，再用同一 TOML 重建）；新 id 无内嵌镜像
> → 500。这是受限的 create/delete：验证运行时 VM 生命周期（内存/vCPU/device 初始化、
> remove 无 task 泄漏、失败回滚），而非任意镜像的运行时加载。对应自测
> `test-suit/axvisor/normal/qemu-http-axum-dynamic/`（`--test-case http-axum-dynamic`）。

> **stop 是异步请求：** 响应带 `"async": true`，POST stop 返回 200 只表示请求被接受，
> `Stopped` 要等 vCPU 退出，返回的 `status` 可能仍是 `running`/`stopping`。立即 GET
> 可能仍是 `stopping`。需等数秒再 GET，或循环 curl 直到状态稳定。
> **start-on-stopped 返回 409**：restart-after-stop 是已知调度限制（stopped VM 再
> start 时新 vCPU task 在其固定核已 idle 后不会被调度），API 契约显式拒绝（409），
> 不会让 VM 挂起在 `running`。

---

## 3. x86_64 变体（UEFI 引导）

x86_64 需要 OVMF UEFI 引导，产物含 PE32+ EFI app。步骤（工作区根目录执行）：

```bash
# 1. 由测试 harness 刷新产物（x86_64 case 用 `-cpu host,+vmx-*`，需 Intel KVM 主机）
cargo xtask axvisor test qemu --test-case http-axum-readonly --arch x86_64

# 2. 手工引导 + hostfwd
cp /usr/share/OVMF/OVMF_VARS_4M.fd /tmp/axvisor-manual.vars.fd
qemu-system-x86_64 -no-user-config -display none -serial stdio -monitor none \
  -cpu host,-la57,+vmx-ept,+vmx-unrestricted-guest,+vmx-flexpriority \
  -machine q35,smbus=off,usb=off,graphics=off -smp 2 -accel kvm -m 512M -vga none \
  -drive if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,unit=1,file=/tmp/axvisor-manual.vars.fd \
  -drive format=raw,file=fat:rw:target/x86_64-unknown-linux-musl/release/axvisor.esp \
  -netdev user,id=net0,hostfwd=tcp::18080-:8080 \
  -device virtio-net-pci,netdev=net0

# 3. 验证（只读端点开放；写端点需要 Bearer token）
curl -s -i http://localhost:18080/api/vms      # 200
curl -s -i http://localhost:18080/api/vms/999  # 404
curl -s -i -X POST http://localhost:18080/api/vms/1/start  # 401（readonly 产物未烘焙 token，写默认拒绝）
```

---

## 4. 清理

手动 QEMU 验证后必须 kill 掉残留实例，否则旧实例占住 hostfwd 端口并持续烧 CPU：

```bash
pkill -f qemu-system
```

---

## 5. 安全边界

控制面采用 **认证 + 受限监听** 双层防护：

- **写接口强制认证（Bearer token）：** `create`/`delete`/`start`/`stop` 以及测试专用
  `POST /__probe_result` 都要求 `Authorization: Bearer <token>` 头，token 在构建期经
  `[env] AXVM_HTTP_TOKEN` 注入（`option_env!` 读取，与 `shell/command/base.rs` 读
  `AX_ARCH` 的机制一致）。**默认拒绝**：构建时未设置该变量则所有写路由一律 `401`，
  没有"回退为允许写入 + 警告"的路径 —— 忘记配置 token 的构建会在测试中直接失败。
- **受限监听（默认 loopback）：** 服务器默认绑定 `127.0.0.1:8080`，管理网络不可达。
  需要 hostfwd/对外暴露的构建必须显式设置 `[env] AXVM_HTTP_BIND = "0.0.0.0:8080"` 才
  会监听所有接口；即便如此，写接口仍受 token 保护。
- **读接口开放：** `GET` 列表/明细是只读的，用于管理面板/调试仪表盘，不做状态变更。
  默认 loopback 绑定下管理网络无法访问；对外暴露时仅能枚举 VM 清单（无变更能力）。
- **仍无加密：** HTTP 明文。在 bare-metal hypervisor 内启用 TLS 超出本期范围；如需暴露
  到不受信网络，请确保 token 强度、在可信网络段内使用，并考虑前置带 TLS 的反向代理。
- **与 IVC/hypercall 的关系（互补）：** HTTP 是 **host 管理员**对外入口；IVC
  （`IvcChannelFactory`）和 hypercall 是 **guest ↔ hypervisor** 内部通道。二者是不同
  方向的通信机制，不互相替代。

---

## 6. 常见问题

| 现象 | 原因 / 处理 |
|------|------------|
| curl 超时/拒绝连接 | QEMU 未启动或 `hostfwd` 端口被旧实例占用。`pkill -f qemu-system` 后重启。 |
| 启动日志无 `use NIC 0:` | `net` 能力由 `ax-std` 无条件启用（不通过 axvisor build config 开关），缺该日志说明 QEMU 没给虚拟机提供网卡——检查 `-netdev user` + `-device virtio-net-pci` 是否都在，或 `hostfwd` 端口是否被残留实例占用（`pkill -f qemu-system` 后重启）。 |
| 无 `management HTTP server` 日志 | `http-axum` feature 缺失——`http::serve()` 整个模块在无该 feature 时不编译。config 必须含 `http-axum`（见 §2.1）。 |
| 启动后无 VM（`GET /api/vms` 空数组） | 构建用 `http-test` 自测产物（VM 被 stop 且自测驱动过生命周期）；按 §2.1 重生成 `.bin`。 |
| 启动日志报 `VM[1] VCpu[0] run ... error ... VGIC ... Distributor write ... register requires Dword` | 预置 guest 镜像（`arceos-qemu`）在 GIC 初始化处做 byte 宽 GICD 写，VGIC 模拟拒绝。**与 HTTP 控制面无关**（vCPU 侧 device 错误），VM 仍会经 Fault 路径转为 `stopped`，控制流程不受影响。 |
| 幂等 stop 时日志出现 `Stopping VM[1]: Forced`（前缀 `0:12` = HTTP 任务） | 正常。`stop_vm` 一律用 `StopReason::Forced`（runtime/mod.rs:128）；对已 Stopped VM，`request_stop_with` 是幂等 no-op（machine.rs:322-333），返回 200。`Forced` 是 stop 的标准 reason，不是错误。 |
| `POST start` 对 stopped VM 返回 409 | 正常，是契约行为。restart-after-stop 是已知调度限制（stopped VM 再 start 时新 vCPU task 在已 idle 的固定核上不被调度），`vm_action` 显式以 409 拒绝，避免 VM 挂起在 `running`。 |
| `POST /api/vms/create` 返回 500 | create 的 TOML `base.id` 没有构建期内嵌镜像（新 id）→ 运行时 `memory_images_for_vm` 报 NotFound。create 只能复现已内嵌且未注册的 id（先 DELETE 再 create）；TOML 解析失败 → 400，id 已注册 → 409。 |
| `DELETE /api/vms/{id}` 返回 500 | `vm.destroy()` 失败（VM 停在 `Destroying`）。handler 先 destroy 后 remove，destroy 失败时 VM 仍在注册表内，可重试 DELETE。 |
| 动态自测日志出现 `VM[1] vCPU runtime cleanup skipped: InvalidState` | 正常。删除从未 start 的 VM 时 `join_all_vcpu_tasks` 无 vCPU runtime 可 join（`cleanup_vm_vcpus` 的 `warn!` 路径），资源仍正确释放（`resources cleanup completed`）。 |
