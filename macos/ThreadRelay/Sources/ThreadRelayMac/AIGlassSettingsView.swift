import SwiftUI

/// ThreadRelay 的本机使用量与通知设置。悬浮窗相关功能不在本项目中启用。
struct AIGlassSettingsView: View {
    @Bindable var settings: AppSettings

    var body: some View {
        Form {
            Section("Codex 使用量") {
                Label("Codex", systemImage: "terminal")
                Text("ThreadRelay 只读取本机 Codex 会话日志，不会修改 Codex 或后台服务。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("额度提醒") {
                Toggle("启用系统通知", isOn: $settings.notificationsEnabled)
                LabeledContent("提醒线") {
                    HStack {
                        Slider(value: $settings.warnThreshold, in: 1...99, step: 1)
                        Text("\(Int(settings.warnThreshold))%")
                            .monospacedDigit()
                            .frame(width: 42, alignment: .trailing)
                    }
                }
                LabeledContent("严重线") {
                    HStack {
                        Slider(value: $settings.critThreshold, in: 1...100, step: 1)
                        Text("\(Int(settings.critThreshold))%")
                            .monospacedDigit()
                            .frame(width: 42, alignment: .trailing)
                    }
                }
            }

            Section("菜单栏显示") {
                Toggle("今日 token", isOn: itemBinding(.todayTokens))
                Toggle("消耗速度", isOn: itemBinding(.burnRate))
                Toggle("使用率", isOn: itemBinding(.usagePercent))
                Toggle("重置倒计时", isOn: itemBinding(.resetCountdown))
                Text("点击菜单栏图标可查看完整的概览、趋势、项目和记录。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("通知类型") {
                Toggle("额度接近上限", isOn: $settings.notifyLimitThreshold)
                Toggle("预计即将耗尽", isOn: $settings.notifyDepletion)
                Toggle("额度窗口重置", isOn: $settings.notifyWindowReset)
                Toggle("消耗速度突增", isOn: $settings.notifyBurnSpike)
                Toggle("回来继续工作", isOn: $settings.notifyComeback)
                Toggle("时段摘要", isOn: $settings.notifyBriefing)
                Toggle("里程碑和新纪录", isOn: milestoneBinding)
            }

            Section("摘要与通知样式") {
                Toggle("拟人化提示语", isOn: $settings.realMode)
                Toggle("时段摘要包含连续使用天数", isOn: $settings.funStreak)
                Toggle("周一显示上周报告", isOn: $settings.funWeeklyReport)
                Toggle("提示音", isOn: $settings.funSoundEnabled)
                Toggle("检测新版本", isOn: $settings.notifyUpdate)
            }

            Section("自定义通知文案") {
                Text("每行写一条候选文案；支持 {AGENT}、{USAGE}、{TOKENS} 和 {RESET} 占位符。留空则使用内置文案。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                ForEach(CustomizableEvent.allCases) { event in
                    DisclosureGroup(event.label) {
                        TextEditor(text: customMessageBinding(for: event))
                            .font(.callout)
                            .frame(minHeight: 58, maxHeight: 100)
                            .overlay {
                                RoundedRectangle(cornerRadius: 6)
                                    .stroke(.quaternary, lineWidth: 1)
                            }
                        Text("默认：(event.sampleDefaultTitle)")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                }
            }

            Section("其他") {
                Toggle("登录时自动启动", isOn: Binding(
                    get: { LaunchAtLogin.isEnabled },
                    set: { newValue in
                        settings.launchAtLogin = newValue
                        LaunchAtLogin.set(newValue)
                    }))
                Text("通知会显示在系统通知中心和菜单栏记录中，不会创建额外的悬浮窗。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .scrollIndicators(.never)
        .padding(12)
    }

    private func itemBinding(_ item: MenubarItem) -> Binding<Bool> {
        Binding(
            get: { settings.menubarItems.contains(item) },
            set: { enabled in
                if enabled { settings.menubarItems.insert(item) }
                else { settings.menubarItems.remove(item) }
            })
    }

    private var milestoneBinding: Binding<Bool> {
        Binding(
            get: { settings.funMilestone && settings.funRecord },
            set: { value in
                settings.funMilestone = value
                settings.funRecord = value
            })
    }

    private func customMessageBinding(for event: CustomizableEvent) -> Binding<String> {
        Binding(
            get: {
                settings.customMessages[event.rawValue]?.messages.joined(separator: "\n") ?? ""
            },
            set: { text in
                var messages = text.components(separatedBy: .newlines)
                    .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                messages = Array(messages.prefix(12))
                var all = settings.customMessages
                if messages.isEmpty {
                    all.removeValue(forKey: event.rawValue)
                } else {
                    all[event.rawValue] = CustomMessageConfig(messages: messages)
                }
                settings.customMessages = all
            })
    }

}
