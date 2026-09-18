//! Management shell panel: the hypervisor's own command interpreter.
//!
//! The web shell and the board's serial console drive the *same* interpreter
//! (`shell/mod.rs`), so a command typed here — `vm list`, `vm pool`, `lspci` —
//! behaves exactly as it does on the physical console. Output does not: a
//! network session's output goes to this lane only (`submit_shell_bytes` routes
//! it before the board console sees it), which is why driving the dashboard
//! never takes the serial console away. The lane is exclusive: the second
//! subscriber gets 409.

import { endpoints } from '@/api/endpoints'
import { TerminalView } from '@/components/Terminal'

export default function ShellPanel() {
  return (
    <div className="flex h-[70vh] min-h-0 flex-col gap-2">
      <p className="text-sm text-muted-foreground">
        {endpoints.managementTerminal} 与板载串口共用同一个解释器：这里输入的 `vm list`、`vm pool`
        等命令和串口上完全一致。会话输出只发到本通道，串口保持自己的输入输出；该通道独占，第二个订阅者会收到 409。
      </p>
      <div className="min-h-0 flex-1">
        <TerminalView path={endpoints.managementTerminal} title="axvisor shell" subtitle="宿主管理终端" />
      </div>
    </div>
  )
}
