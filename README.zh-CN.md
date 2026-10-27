# cpp-infra-style

[English](README.md)

`cpp-infra-style` 在构建前检查配置范围内的全部 C/C++ 源文件。它显式使用统一的
clang-format 公共策略，并检查 SPDX 文件头、末尾换行和行尾空白。新建且尚未跟踪的
文件同样参与检查，无须配置 CMake 或生成编译数据库。

构建需要 Rust 1.94，检查需要 clang-format 18。交付策略采用 LLVM 格式、两空格缩进、
100 列限制、LF 行尾，要求前八行包含 `Apache-2.0` SPDX 标识与 `Copyright (c)`
版权注释。项目及策略路径均限制在 canonical 项目根目录内。

```sh
cargo build --locked
cpp-infra-style --project /path/to/project plan
cpp-infra-style --project /path/to/project check
cpp-infra-style --project /path/to/project fix --dry-run
cpp-infra-style --project /path/to/project fix
```

项目必须包含 `.infra/project.toml`：

```toml
schema = 1
kind = "library"
source_roots = ["src", "include"]
test_roots = ["tests"]
generated_roots = []
third_party_roots = []
```

只声明实际存在、归属互不重叠的目录。配置 `.infra/style/config.toml`：

```toml
schema = 1
extensions = ["h", "hpp", "hxx", "c", "cc", "cpp", "cxx", "ipp", "tcc"]
include = ["src/**", "include/**", "tests/**"]
exclude = []
rules = ["format", "spdx-header", "final-newline", "trailing-whitespace"]
exceptions = ".infra/style/exceptions.toml"
```

工具集将 `conf/policy/rules.toml` 和 `clang-format.yaml` 安装到
`.infra/style/policy/`。manifest 的 `project_files` 还声明 `conf/config.toml` 为
项目 `.infra/style/config.toml` 初始模板：`init` 仅在缺失时创建，普通 update 保留
项目修改。模板 include 使用 `**` 与项目实际归属根取交集，不假设源码目录名称。
项目不能删除组织强制规则，嵌套 `.clang-format` 不会改变
检查行为。扫描范围为项目源码/测试目录与 include 的交集，再排除 exclude。
生成与第三方归属目录排除；`.git`、`.infra`、`build` 和 `target` 仅排除项目根层，
源码内嵌套同名目录仍参与检查。项目内符号
链接不跟随，越界链接报错。不会读取 Git ignore。空集合及非 UTF-8 源码报错。
POSIX 项目输入路径中的字面反斜杠会被拒绝，不会转换为目录分隔符或例外路径别名。
实际使用的 project、style、policy、formatter 与 exceptions 配置必须是受公共
指纹跟踪的普通文件；词法与 canonical 位置均不得落在根 `.git`、`build`、`target`
或 `.infra/.tmp`、`.infra/transactions`、`.infra/.transactions` 中。配置文件及父目录
符号链接会明确拒绝，因为公共 no-follow 指纹不记录链接身份，无法认证重定向。
该限制不用于运行上下文、快照或报告；普通 `src/build` 等受跟踪位置中的自定义
例外文件仍可使用。
加载时单独记录每个实际配置文件的身份与消费字节摘要；格式化前后、替换前和最终
完整检查均复验，包含 `.infra` 外的自定义例外文件。`fix` 不会把运行中策略变化
吸收到新的源码摘要中，也不会重载新例外后认证旧上下文；被源码范围选中的实际
配置文件同样不会被 `fix` 改写。

可选的精确例外文件：

```toml
schema = 1
[[exceptions]]
rule = "spdx-header"
path = "src/compat.cpp"
reason = "必须保留原始版权和许可证声明。"
```

例外必须引用已启用、允许豁免的已知规则，路径必须精确且存在，原因不能为空。
`format` 不可豁免。未知字段、通配路径、重复条目、未匹配任何诊断的失效例外
均报配置错误。报告保留被压制的诊断与匹配数量。

`check` 不写源码。`fix` 仅运行 clang-format，保留文件权限，每次原子替换前
重新核对原始字节；遇到并发编辑立即停止，不覆盖新内容。多个文件并非单一事务，
报告记录已完成与未完成文件。修复后重新运行完整检查，其余机械规则仍失败时
返回门禁失败。dry-run 输出统一差异，不写源码。formatter 故障不可豁免。

每次执行均写公共协议 1 报告，默认位于
`build/infra-runs/<run-id>/style-report.json`，可用 `--report <path.json>` 指定。
命令日志及补丁产物位于 `report_root` 下并记录 SHA-256。
每次调用在共享根目录下独占创建 `style/<unique-call-id>/`，日志及补丁不会覆盖
其他任务或重试的工件。不安全的输出祖先目录在创建文件前拒绝；没有安全的报告
目标时，只向 stderr 输出错误。
`--run-context <json>` 校验 canonical 项目身份、本次运行的工具集身份、源码及配置摘要，
过期上下文失败。本次工具集可以跟踪 cpp-infra main，无须与项目持久 tools 锁相同。
可选 `source_excluded_paths` 为排序去重的精确项目相对 JSON 报告文件清单。
项目内根 `build`/`target` 之外的 `--report` 在验证非输入属性后合入清单。
源码、测试、生成、第三方、基础设施、CMake、Conan 与 README 输入均不可排除；
已有文件必须是 cpp-infra 公共报告，未知业务 JSON 不覆盖。初始化与最终源码指纹
复验使用同一清单，配置指纹不受其影响。配置事务仅忽略根 `.infra/.tmp`、
`.infra/transactions`、`.infra/.transactions`，其他层级或相似名称仍参与指纹。
项目基础设施仅在显式 update 时同步。`--offline` 不触发网络访问。`--timeout-seconds` 限制单次
formatter 运行时间，默认 30 秒。Unix 上 Ctrl-C 终止 formatter 进程组并写取消报告；
Windows 使用 `taskkill /PID <pid> /T /F` 终止 formatter 及其子孙进程。
`CPP_INFRA_CLANG_FORMAT` 可选择 formatter 可执行文件，否则使用 PATH 中的
`clang-format`，仍强制校验版本。未发布源码构建的 `--version --json`
返回 `revision: null`。

上下文提供 `runtime_snapshot` 和 `runtime_snapshot_digest` 时，检查快照 SHA-256、
工具集身份、自身版本/revision/协议，以及缓存与当前执行文件的摘要。
`CPP_INFRA_BIN_DIR` 中的副本路径可以不同，但文件内容必须与快照一致。

退出码：0 通过、1 风格门禁失败、2 配置/协议/版本错误、3 文件系统/进程故障、
130 取消。`plan` 不启动外部工具，输出 JSON 并记录 `plan` 模式，不表示风格检查通过。

```sh
cargo test --locked
cargo +nightly fmt --all -- --check --config-path .infra/style/rustfmt.toml
cargo clippy --all-targets -- -D warnings
```

测试使用隔离项目，正常流程调用真实 clang-format 18；崩溃、超时及并发编辑使用
受控进程 fixture，不测试下游仓库。
