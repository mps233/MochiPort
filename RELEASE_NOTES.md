# CodexHub v0.2.16

这是 `v0.2.15` 之后的一个小版本，主要修复 macOS 上 AI Gateway 渠道列表过高时，渠道操作入口需要滚动到底部才能看到的问题。

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。

## 更新内容

- 将 AI Gateway 渠道列表的“新增渠道 / 编辑渠道 / 删除渠道”三个入口移动到列表上方。
- 在 macOS 端渠道列表较高时，用户不再需要滚动到底部才能管理渠道。

## 验证

- `cargo fmt`
- `cargo build --release --features gui --bin codexhub`
