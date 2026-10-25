# RTSS 与 WebView2 的进程级兼容

本程序在 `GuiProcessController._spawn_locked()` 复制的 GUI 子进程环境中设置
`RTSSHooksCompatibility=0`，不修改托盘进程的环境。`run_gui()` 在原生初始化前
设置同一声明，覆盖直接运行 GUI 的入口。WebView2 继承 GUI 环境，作用范围是本程序
GUI 及其后续子进程。应用不写 RTSS Profiles，不写用户或系统环境变量注册表项。

首次声明时，通过私有进程环境字段 `MOUSE_BATTERY_RTSS_ORIGINAL_ENV` 保存原值
（JSON null 表示原本未设置）。重复声明保留首次记录。`updater.download_and_install()`
启动 PowerShell 助手前，只在传入助手的环境副本中还原原值并移除私有字段；
因此更新成功及回滚重启的托盘均继承原环境，当前 GUI 仍保持禁用声明。

## 来源与适用边界

- [RTSS 作者关于兼容声明的说明](https://forums.guru3d.com/threads/atomic-heart-vk-play-version-injection.446759/)确认可通过环境变量关闭进程的 Hook，也说明 `ForceHooking=1` 可以覆盖声明。
- [RTSS 作者的版本变更说明](https://forums.guru3d.com/threads/msi-ab-rtss-development-news-thread.412822/page-194)说明 7.3.4 Beta 9 增加了针对 VK Play 启动器滥用继承的检测。不能推断所有版本都会无条件接受任何继承值。
- [Sodium 开发者的调查](https://github.com/CaffeineMC/sodium/issues/2263)提到该环境变量，但其 `=1` 说法与本机 7.3.5 实测不符，不能照抄。
- [RTSS 作者关于导出变量的说明](https://forums.guru3d.com/threads/rivatuner-statistics-server-7-3-3-incompatible-with-latest-ms-powertoys-v0-58-0.442873/)给出 EXE 导出 `DWORD RTSSHooksCompatibility=0` 的另一条路径。本程序不修改微软 WebView2 EXE 或自行注入 DLL。
- [Windows 进程环境文档](https://learn.microsoft.com/en-us/windows/win32/procthread/environment-variables)说明默认子进程继承与显式传递环境块的作用范围。
- [RTSS 作者关于 CBT 的说明](https://forums.guru3d.com/threads/rtss-hooking-into-blacklisted-applications.421244/)解释：关闭 API Hook 不等于阻止系统加载 `RTSSHooks64.dll`。模块存在本身不能作为失败信号。

这是兼容请求，不能宣称适用于所有 RTSS 版本，或能够修复所有 `BrowserProcessExited`。
若用户启用了 ForceHooking，程序不改写其设置；故障仍由有限重试、隐藏静默回收及托盘通知处理。

## 2026-10-25 本机验证

环境：Windows x64，RTSS 7.3.5.28314。测试使用独立临时进程与隐藏窗口，退出后释放；
未修改 RTSS 配置。RTSSHooks64.dll SHA256：
`68C496DEA7DED5B8766092F0159F4E9BA3947DF0829F594EB6981D2B06352AD5`。

先只读检查该版本 DLL：读取 `RTSSHooksCompatibility` 后以 `%d` 解析，值为零才将
当前进程的有效 Hooking 标志清零。随后在三个独立进程中触发系统 CBT，读取其有效状态：

| 启动时的进程环境变量 | 有效 Hooking 状态 |
| --- | --- |
| 未设置 | 1，开启 |
| `1` | 1，开启 |
| `0` | 0，关闭 |

此状态位的位置只用于本次验证，不进入产品实现，不能套用到其他 RTSS 构建。
RTSS SDK 的 `LoadProfile` / `GetProfileProperty` 读取的是编辑用配置对象，不能用它
返回的 Application detection level 判断进程兼容声明是否生效。

再以 `0` 启动真实 pywebview / WebView2 隐藏窗口，枚举其运行进程并读取相同版本的
有效状态：Browser 与一个 Utility 进程加载了 RTSS DLL，Hooking 均为 0；
GPU、Renderer 和另一 Utility 未加载 RTSS DLL。网页加载完成，窗口正常销毁，进程退出码 0。
这验证了本机实际 WebView2 链路，未构成跨版本或长期不崩溃的保证。

## 回归验证

运行 `python -m pytest -q tests/test_rtss_compat.py tests/test_gui_recovery.py`。
新增测试通过 Win32 `GetEnvironmentVariableW` 读取实际 GUI 启动环境及下一代子进程环境，
并确认托盘原环境保持不变；另外验证直接启动 GUI 时声明早于原生初始化。
更新助手测试覆盖原变量未设置、原值为 1、原值为 0，以及声明被重复设置；
两条重启路径均从还原后的助手环境继承，不更改 GUI 当前环境。
现有测试继续覆盖唤醒请求保存、故障与消息领取交错、重试上限、退出优先级和非模态通知。
