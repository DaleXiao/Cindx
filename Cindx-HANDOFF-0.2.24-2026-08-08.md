# Cindx 0.2.24 Handoff

日期：`2026-08-08`

## 1. 用途与边界

这是 `v0.2.24` 的一次性交接快照，供下一位 Cindx 维护者开始后续 Goal
前核对事实。实现以源码为准；当前产品、架构、工具和评估契约仍分别以
[`docs/CURRENT.md`](docs/CURRENT.md)、
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)、
[`docs/TOOL_HARNESS_SPEC.md`](docs/TOOL_HARNESS_SPEC.md) 和
[`docs/AGENT_EVALUATION.md`](docs/AGENT_EVALUATION.md) 为准。

本文件不建立第二套架构说明或路线图。接手者负责在开始工作时重新核对
远端、依赖和运行环境；`main` 出现新的产品提交后，本文件只保留历史交接
价值，不再代表当前状态。

维护责任：开始下一 Goal 的 Cindx 维护者负责复核本快照；验证边界仅覆盖
下列精确 revision、已记录门禁和 Release 元数据。

## 2. 精确发布身份

| 项目 | 已核对事实 |
| --- | --- |
| 应用版本 | `0.2.24` |
| Release tag/target 提交 | `c7b1ba694daef2c7d1c571080a2f245431818aa6` |
| 打包实现源码 revision | `748df262760327a5f27a21e9447c8bac6cfe4370` |
| 标签 | `v0.2.24`，指向上述提交 |
| GitHub Release | [Cindx v0.2.24](https://github.com/DaleXiao/Cindx/releases/tag/v0.2.24)，prerelease |
| 已发布附件 | `Cindx-0.2.24-macOS-arm64.zip`，`53,738,192` bytes |
| 附件 SHA-256 | `12b7280a43d2d79855bf947fabb80a15a2893a26ad30093d88ff7139de222f1c` |
| 本机安装 | `/Applications/Cindx.app`，版本 `0.2.24`，arm64 |
| 本机签名 | ad-hoc；`codesign --verify --deep --strict` 通过；未公证 |

Release 提交 `c7b1ba6` 只同步六个版本文件。已安装二进制内嵌的实现源码
revision 是 `748df262760327a5f27a21e9447c8bac6cfe4370`，即其直接父提交；
两者之间没有产品实现差异。此前发布核对已确认远端附件与本地打包文件逐
字节一致。

## 3. 本版本交付的 Agent 改进

### Goal 1：闭合可验证的 patch 完成路径

提交：`911fb9a3255658f9ebb93a6fb466ec3287faead9`

- `file.patch` 的动态 after-SHA 绑定 patch 与恢复事实；只有可信 exact
  readback 或 workspace-wide quality check 才能签发 postcondition receipt。
- 中断恢复只有在当前文件仍与记录的 after-SHA 一致时才确认已应用；否则
  fail closed，不重复写入。
- 作用是减少“文件已经正确修改但 Agent 仍继续循环”与恢复时重复副作用，
  没有放宽权限、成功或验证标准。

### Goal 2：识别非相邻的语义重复只读结果

提交：`51d2399817270cab1e5bfa933584ceb20c0c82ba`

- adaptive loop 对可信、完整、动态只读 observation 保留有界语义身份。
- 相同 action 再次得到相同内容时只请求一次有界 replan；内容变化会重置
  no-gain 信号。
- 不缓存或跳过工具执行，不授予 Goal Delta，不跨越 effect、失败、拒绝、
  取消或不完整 observation。

### Goal 3：让 Direct Finalizer 策略具备可归因学习面

机制提交：`2647daafae98e99d31c101886d2836e446d83d96`

- 增加一个正交、仅作用于 Auto/Pro Direct 路径 tools-disabled Finalizer 的
  prompt gene。
- seed/evidence phenotype 不增加指令，因此默认产品行为保持不变。
- assignment、request、delivery、reviewer、GEPA、holdout 和 provider-call
  预算均有类型化、fail-closed 的因果边界。

证据保真修复：`748df262760327a5f27a21e9447c8bac6cfe4370`

- Gate A 被阻断时仍必须保留精确 candidate profile identity。
- 修复没有改分数、provider 输出 digest、调用计数或决策。

## 4. 验证证据与可声明结论

### 确定性与工程门禁

实现门禁记录在 `748df26`；随后 `c7b1ba6` 只改六个版本文件。该实现 revision
实际通过：

- `cargo fmt --all -- --check`
- Direct-finalizer desktop tests：`24/24`
- Direct-finalizer orchestrator/evolution tests：`9/9`
- `bash scripts/check-rust-quality.sh`
- `node scripts/check-docs.mjs`
- `node scripts/check-desktop-structure.mjs`
- `git diff --check`
- retained JSON 解析和敏感内容检查
- 完整非 sandbox 质量 profile：browser sidecar、workspace tests、desktop
  `703 passed / 7 ignored`、frontend `141`、Direct-finalizer `24/24`、
  evolution `9/9`、shipping-performance 全部通过

在 release 提交 `c7b1ba6` 上另行通过了 `node scripts/check-docs.mjs`、
`node scripts/check-release-version.mjs v0.2.24`、
`node scripts/check-desktop-structure.mjs` 和 `git diff --check`。

同一完整 profile 首次在 sandbox 内只因 loopback `127.0.0.1` 被环境以
`EPERM` 拒绝而失败；在非 sandbox 下按相同 profile 复跑通过。这不是产品
回归证据。

上述结果证明契约、回归和资源门禁，不证明模型质量或 Fugu Ultra 对齐。

### Provider-backed Direct-finalizer 校准

权威报告：
[`CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.md`](docs/evaluations/CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.md)

没有单独的 `0.2.24` provider-backed 报告；不能把下面的 `0.2.23` 定向实验
改名为 `0.2.24` 质量结论。

- treatment source：`2647daa`；provider：DashScope `alibaba_cn`
- 4 个 preregistered Gate A pairs 全部保留
- 16 个 provider receipts；20 个保守 call reservations
- parent deterministic pass：`2/4`；candidate：`1/4`
- candidate 在 `train-test-failure` preservation case 回归
- 决策：`VALID_TARGETED_EVIDENCE`、`NO_GO_FOR_PROMOTION`
- GEPA attempts：`0`；没有 holdout、snapshot、promotion、deployment 或 transfer
- shipping seed 未改变

这证明门禁能拒绝“reviewer 看起来更好但客观契约退化”的候选，不能声明
Direct-finalizer、Auto、Pro 或 GEPA 的智能水平已经提高。最新完整产品级
provider baseline 仍是 `0.2.22` V5；它只支持 adaptive-direct `IMPROVED`，
不是 capability GO，workflow、learned-profile 和 distillation 均未执行，
Pro 仍是描述性结果。其适用边界见
[`docs/AGENT_EVALUATION.md`](docs/AGENT_EVALUATION.md)。

## 5. Release 的已知缺口

GitHub Release run
[`31171772960`](https://github.com/DaleXiao/Cindx/actions/runs/31171772960)
在 `Audit frontend dependency graph` 失败：当前 `mermaid 11.16.0` 命中
5 个 advisories（2 项 prototype pollution、1 项 CSS injection、2 项 diagram
DoS），被 npm 汇总为 1 个 moderate vulnerability。因此：

- release source tests 和 shipping-performance 没有在该 workflow 中执行；
- `Upload shipping performance report` 因报告文件不存在而次生失败；
- Universal build/publish 全部被跳过；
- 当前 Release 是后来上传并核对的 arm64 包，不是
  [`docs/RELEASING.md`](docs/RELEASING.md) 所描述的 Universal workflow 产物。

另有一个尚未被本次 run 触发的静态风险：CI workflow 安装 `protobuf`，
Release workflow 当前没有同一步骤，却会运行默认特性的 Cargo tests。只能把
它视为后续阶段风险，不能称为本次失败原因。

## 6. 建议的后续顺序

1. **先恢复发布链。** 以独立、有限 Goal 升级 Mermaid 到已修复版本，针对
   Mermaid 渲染、最大化交互、主题和现有 Markdown 路径做回归，再跑
   `npm audit`。同时核对 Release 与 CI 的系统依赖差异。目标是让现有
   Universal workflow 完整通过，不能用降低审计级别或忽略 advisory 代替。
2. **再改进失败候选，不推广它。** 从 `train-test-failure` 的客观回归建立新
   的冻结候选；保持 parent、provider、budget、context、reviewer 和失败分母
   一致。Gate A 通过前不进入 GEPA 或 holdout。
3. **只有出现合格 snapshot 后再做 serving admission。** 当前没有可部署候选，
   不应预先增加第二条 serving path。届时应复用既有 rollout/canary/CAS/
   rollback，并以版本化、fail-closed import/admission 接入。
4. **扩大能力声明前重跑产品级 matched evidence。** 确定性绿色不能替代
   provider-backed Auto/Pro、workflow、learned-profile 或 distillation 证据。

## 7. 接手检查清单

开始任何新 Goal 前：

```sh
git pull --ff-only
git status --short --branch
git log -8 --oneline --decorate
node scripts/check-docs.mjs
node scripts/check-release-version.mjs v0.2.24
```

- 重新确认远端 `main`、目标 tag、当前应用版本和工作区状态，不依据旧 Goal
  tracker 推断进度。
- 先读本文件第 1 节列出的权威文档和 concern-specific 文档。
- 每个 Goal 只修其 scope 内的问题；复核不得借机扩大范围。
- provider run 需要单独授权、冻结 protocol 和明确调用预算。
- 发布前按 [`docs/QUALITY_GATES.md`](docs/QUALITY_GATES.md) 执行相应 profile；
  上传后核对 remote SHA、asset digest、安装版本、架构和签名。
- 清理只删除已确认的构建/临时产物；保留 `.cindx`、用户配置、Git 状态和
  已安装应用。
