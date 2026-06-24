# 项目更名:codex-remote → CodexHub

本文记录 `codex-remote` 更名为 `CodexHub` 的决策、改动范围、刻意保留项、验证结果，以及仍需手动完成的步骤。供后续维护和回溯参考。

## 背景

原名 `codex-remote` 把项目说小了。它的实质是在本地自建一个 Codex 后台：远程 IM 控制只是其中一条线，本地 AI Gateway（多模型入口）甚至更有分量。原名既不吸引眼球，也没有把"本地后台 + 多模型 + 多端"这层定位讲出来。

## 命名决策

最终定名 **CodexHub**。考量如下：

- 保留 `Codex` 词根：项目定位是"增强 Codex 官方能力"，绑定商标词根能对上用户心智、利于检索；完全去掉反而容易被忽略。
- 选择 `Hub`（本地总控台）而非 `Max` / `++` 之类"更强版"后缀：避免被解读成蹭热度，气质更独立，也准确表达"一个枢纽把远程控制、模型网关、会话管理收在一起"。
- 商标边界：靠词根 + tagline 说明"非官方/兼容增强"，不使用官方配色，不暗示 official。

三层命名一次到位：

1. 产品显示名：`CodexHub`
2. crate / 二进制：`codexhub`
3. 命令行：`codexhub`

## 改动范围

本次为全量更名，项目无历史用户，因此不保留任何向后兼容逻辑（状态文件、备份目录、环境变量、握手标识一律直接改新名）。

源码（37 个 `.rs` 文件）
- 产品显示名 `Codex Remote` → `CodexHub`
- crate 代号 / 日志 target `codex_remote` → `codexhub`
- 二进制标识、daemon 进程自识别字符串 `codex-remote` → `codexhub`
- 状态文件 `codex-remote-state.json` → `codexhub-state.json`
- App support 备份目录 `Codex Remote` → `CodexHub`
- 环境变量 `CODEX_REMOTE_HOME` / `CODEX_REMOTE_USE_REPO_CONFIG` → `CODEXHUB_HOME` / `CODEXHUB_USE_REPO_CONFIG`
- remote-control / 飞书握手标识：`client_id`、auth title、account/user id 等
- 微信 `bot_agent`、HTTP `User-Agent` 等内嵌标识

构建与打包
- `Cargo.toml`：`name` 与 `[[bin]]` 均改为 `codexhub`
- `build.rs`：链接参数与 manifest / `.rc` 路径
- `packaging/windows/codex-remote.exe.manifest` → `codexhub.exe.manifest`（git mv + 内容）
- `packaging/windows/codex-remote.rc` → `codexhub.rc`（git mv）
- `packaging/linux/codex-remote.desktop` → `codexhub.desktop`（git mv + 内容）
- `packaging/macos/Info.plist`：`CFBundleDisplayName` / `CFBundleExecutable` / `CFBundleName` 改为 `CodexHub`，`CFBundleIdentifier` 改为 `com.codexhub.app`
- 三个 GitHub workflow（`release-macos` / `release-windows` / `release-linux`）：构建 `--bin codexhub`，产物名改为 `CodexHub.dmg` / `CodexHub.exe` / `CodexHub Windows.zip` / `CodexHub Linux x86_64.AppImage`，bundle 内可执行名与 `CFBundleExecutable` 保持一致

文档与元数据
- `README.md` / `README.en.md` / `CONTRIBUTING.md` / `RELEASE_NOTES.md`
- `docs/` 下全部相关文档
- `LICENSE` 版权署名 → `CodexHub contributors`（协议正文未改）
- `config.example.toml` 的 `statePath`
- `.gitignore` 的状态文件忽略项

## 刻意保留（不可改，改了会出问题）

1. `src/codex.rs` 中的 `__codexRemotePermissionsResponse` / `__codexRemoteMcpElicitationResponse`：这是本项目内部约定的审批响应 sentinel key，语义是"远程审批响应"，与产品名无关。其大小写为 camelCase（`codexRemote`），与被替换的所有 token 大小写均不冲突，替换时天然安全。
2. `docs/remote-control-protocol-audit.zh-CN.md` 标题中的 `Codex Remote-Control`：这是官方 remote-control 协议术语，不能变成 `CodexHub-Control`。替换脚本用负向断言 `Codex Remote(?!-Control)` 将其排除。

另外，`references/`、`vendor/`、`target/` 等目录不在更名范围内。

## 验证结果

- `cargo build --bin codexhub`：通过（仅有既有 dead-code 警告）
- `cargo build --features gui --bin codexhub`：通过；GUI 构建同时验证了重命名后的 Windows manifest / `.rc` 路径能正确链接
- `cargo test --bin codexhub`：305 项全部通过，0 失败；remote-control 握手相关测试未受影响
- 全仓库扫描（排除 `vendor` / `target` / `references` / `Cargo.lock`）：无 `codex-remote` / `codex_remote` 残留，唯一保留项为官方术语 `Codex Remote-Control`
- `Cargo.lock` 包名已更新为 `codexhub`

所有改动位于分支 `codex/rename-codexhub`。

## 后续操作（命令行流程）

下列步骤在本地 PowerShell 执行即可，几乎全部可用 `gh` / `git` 命令完成。前置条件：已安装 GitHub CLI（验证环境为 `gh` 2.76.1）并完成 `gh auth login`，账号 `happy-loki`，token scopes 含 `repo` 与 `workflow`。命令中的 `happy-loki` 按实际属主替换。

### 1. 提交并推送当前更名分支

先把 `codex/rename-codexhub` 分支的全部改动提交并推送，再做仓库改名，避免改名后本地状态与远程脱节。

```powershell
cd D:\rust_demo\codex-remote
git add -A
git commit -m "Rename project codex-remote -> CodexHub"
git push -u origin codex/rename-codexhub
```

可选：直接用 `gh` 开 PR 合并到主分支。

```powershell
gh pr create --fill --base main --head codex/rename-codexhub
```

### 2. GitHub 仓库改名

`gh repo rename` 会改远程仓库名，并自动更新当前仓库的 `origin` URL，因此不需要再单独跑 `git remote set-url`。

```powershell
gh repo rename codexhub --repo happy-loki/codex-remote
```

验证改名与 remote 已同步：

```powershell
gh repo view happy-loki/codexhub --json name,url
git remote -v
```

GitHub 会把旧地址 `happy-loki/codex-remote` 自动 301 重定向到新地址（HTTPS / SSH / issue / PR 链接均覆盖）。

### 3. 占住旧名，防止重定向被截胡

重定向是"尽力而为"：一旦有人用旧名 `codex-remote` 新建仓库，旧地址重定向立即失效、指向他人。建议立刻建一个空的同旧名占位仓库占住它。

```powershell
gh repo create happy-loki/codex-remote --public --description "Renamed to CodexHub: https://github.com/happy-loki/codexhub"
```

注意：占位仓库要与改名后的仓库**同名冲突检测无关**——改名已释放旧名，这里是主动重新占用。若提示名称被占用，说明已被他人抢注，此时旧链接已不可控，只能在 README / Releases 里显著标注新地址。

### 4. 本地文件夹改名（可选）

与功能和远程仓库名均无绑定关系，`git remote` 指向 URL 而非目录名，改不改都不影响 push / pull。若要让本地目录与新品牌一致：

```powershell
# 先关闭占用该目录的程序（IDE、终端、cargo / target 锁），再在上一级目录执行
cd D:\rust_demo
Rename-Item codex-remote codexhub
cd D:\rust_demo\codexhub
```

改名后 `target/` 缓存路径变化会触发一次完整重新编译（数分钟）。git 历史、分支、remote 全部随目录一起迁移，不受影响。

### 5. 仍需人工跟进（无法命令行覆盖）

- 已发布 Releases 的资产名（旧的 `Codex Remote.dmg` 等历史产物名不会被回溯改名，新 tag 触发的 workflow 才会产出 `CodexHub.*`）。
- 公众号、外部文档、第三方收录里写死的旧下载链接与 `owner/repo` 链接。
- 若仓库开启了 GitHub Pages 或外部 CI，硬编码的旧仓库名需手动更新。

## 命名分层与仓库名的关系

三层命名（仓库名 / 产品显示名 / 包名与命令名）可以不完全一致，但建议尽量统一以降低认知成本。本地工作目录名与 GitHub 仓库名之间没有绑定关系：`git remote` 指向的是 URL，而非目录名，因此目录叫什么都不影响 push / pull。
