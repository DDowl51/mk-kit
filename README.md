# mk-kit

微内核架构的共享底座：kernel、启动库、测试工具，以及和业务无关的系统服务。从 [lifelog](../lifelog) 抽出（`git subtree`，保留历史），lifelog、stash、dida 用 path 依赖它。

| 目录 | 内容 |
|---|---|
| `crates/` | `boot`、`boot-build`、`kernel`、`kernel-api`、`testkit`；测试工具 `config-doc`（核对配置文档）；纯逻辑库 `zh-dates`（中文日期写法，应用功能的 core 可以依赖） |
| `contracts/` | 系统服务的契约：browser、cli、clock、exec、files、log、notify、process、prompt、screenshot、sql、timer、tui、vcs、window |
| `services/` | 契约的实现：browser-chromium、browser-firefox、browser-history、browser-qutebrowser、cli-clap、clock-system、exec-tokio、files-std、log-stderr、notify-dbus、process-procfs、prompt-vicinae、screenshot-niri、storage-sqlite、timer-tokio、tui-ratatui、vcs-git、wm-niri |
| `tests/arch` | 架构检查：按目录判断包的角色，检查依赖方向 |

命令行服务支持分组：子命令名里的空格表示分组（`dida ls` 是 `dida` 下面的 `ls`），一组命令可以原样挂到另一个程序下面。

这里**不放任何业务**。改契约要考虑所有使用方：加字段用 `#[serde(default)]` 保持兼容，不兼容的改动升契约版本号，并在使用方（lifelog、stash、dida）跑全量测试。

```sh
./scripts/check.sh   # 格式、clippy、架构规则、全部测试
```
