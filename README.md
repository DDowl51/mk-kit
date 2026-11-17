# mk-kit

微内核架构的共享底座：kernel、启动库、测试工具，以及和业务无关的系统服务。从 [lifelog](../lifelog) 抽出（`git subtree`，保留历史），lifelog 和 stash 用 path 依赖它。

| 目录 | 内容 |
|---|---|
| `crates/` | `kernel-api`、`kernel`、`boot`、`boot-build`、`testkit` |
| `contracts/` | 系统服务的契约：browser、cli、clock、files、log、sql、timer、tui、window |
| `services/` | 契约的实现：浏览历史（Zen/Firefox、Chromium、qutebrowser）、cli-clap、clock-system、files-std、log-stderr、storage-sqlite、timer-tokio、tui-ratatui、wm-niri |
| `tests/arch` | 架构检查：按目录判断包的角色，检查依赖方向 |

这里**不放任何业务**。改契约要考虑所有使用方：加字段用 `#[serde(default)]` 保持兼容，不兼容的改动升契约版本号，并在使用方（lifelog、stash）跑全量测试。

```sh
./scripts/check.sh   # 格式、clippy、架构规则、全部测试
```
