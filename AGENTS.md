# 项目协作规则

## 编码

- 不得防御性编程。按明确需求和现有契约直接实现，禁止为未经证实的故障添加兜底分支、重试、重复校验、兼容层或额外配置。
- 在输入边界做契约要求的校验，按现有约定处理已知错误。优先复用项目已有的校验和错误处理。

## 许可证标记

- 新建或修改代码文件时，在文件头按该语言的注释语法补齐版权署名和 SPDX 许可证标记。此规则适用于源文件、脚本、构建配置、工作流、示例和测试。
- 项目作者署名为 `ZGMFX01A`。项目自有代码使用 Wireless Device Battery Monitor License，完整条款见仓库根目录的 [LICENSE](LICENSE)。
- 第三方代码及其派生代码须按实际适用的许可证标记，并保留上游版权和来源说明。

Rust、HLSL 等使用 `//` 注释的文件按以下格式填写。

```rust
// SPDX-FileCopyrightText: 2026 ZGMFX01A
// SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
// See LICENSE in the repository root for the full terms.
```

PowerShell、TOML、YAML 等使用 `#` 注释的文件按以下格式填写。

```text
# SPDX-FileCopyrightText: 2026 ZGMFX01A
# SPDX-License-Identifier: LicenseRef-Wireless-Device-Battery-Monitor
# See LICENSE in the repository root for the full terms.
```
