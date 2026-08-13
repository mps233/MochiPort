import SwiftUI

/// A compact, read-only map of the clients and message channels attached to
/// the local ThreadRelay service. The map intentionally uses geometry for the
/// lines, so it remains legible when the window is resized or text is scaled.
struct ConnectionTopologyView: View {
    @EnvironmentObject private var model: AppModel

    private let nodeHeight: CGFloat = 56
    private let nodeSpacing: CGFloat = 10
    private let topologyHeight: CGFloat = 286

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("连接拓扑")
                        .font(.title3.weight(.semibold))
                        .tracking(-0.2)
                    Text("本地服务与接入端的实时状态")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }

            GeometryReader { proxy in
                topologyLayout(in: proxy.size)
            }
            .frame(minHeight: topologyHeight, idealHeight: topologyHeight, maxHeight: topologyHeight)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overview.connection-topology")
    }

    @ViewBuilder
    private func topologyLayout(in size: CGSize) -> some View {
        let layoutWidth = min(size.width, 660)
        let contentWidth = max(0, layoutWidth - 16)
        let sideWidth = min(190, max(132, contentWidth * 0.27))
        let serviceWidth = min(190, max(166, contentWidth * 0.27))
        let gap = max(12, (contentWidth - sideWidth * 2 - serviceWidth) / 2)
        let layoutSize = CGSize(width: layoutWidth, height: size.height)

        if #available(macOS 26.0, *) {
            GlassEffectContainer(spacing: 12) {
                topologyStack(
                    size: layoutSize,
                    sideWidth: sideWidth,
                    serviceWidth: serviceWidth,
                    gap: gap
                )
            }
            .frame(width: layoutWidth, height: size.height)
            .frame(maxWidth: .infinity)
        } else {
            topologyStack(
                size: layoutSize,
                sideWidth: sideWidth,
                serviceWidth: serviceWidth,
                gap: gap
            )
            .frame(width: layoutWidth, height: size.height)
            .frame(maxWidth: .infinity)
        }
    }

    private func topologyStack(
        size: CGSize,
        sideWidth: CGFloat,
        serviceWidth: CGFloat,
        gap: CGFloat
    ) -> some View {
        ZStack {
            TopologyConnectorCanvas(
                size: size,
                leftCount: leftNodes.count,
                rightCount: rightNodes.count,
                nodeHeight: nodeHeight,
                nodeSpacing: nodeSpacing,
                sideWidth: sideWidth,
                serviceWidth: serviceWidth,
                gap: gap,
                layoutPadding: 8,
                leftTint: remoteTint,
                rightTint: bridgeTint,
                leftBranchTints: leftNodes.map(\.tint),
                rightBranchTints: rightNodes.map(\.tint)
            )

            HStack(alignment: .center, spacing: gap) {
                nodeColumn(leftNodes, width: sideWidth, side: .left)
                TopologyServiceNode(
                    status: model.serviceStatus,
                    remoteTint: remoteTint,
                    bridgeTint: bridgeTint
                )
                .frame(width: serviceWidth)
                .accessibilityIdentifier("topology.node.local-service")
                nodeColumn(rightNodes, width: sideWidth, side: .right)
            }
            .padding(.horizontal, 8)
        }
        .frame(width: size.width, height: size.height)
    }

    private func nodeColumn(
        _ nodes: [TopologyNode],
        width: CGFloat,
        side: TopologyNodeSide
    ) -> some View {
        VStack(spacing: nodeSpacing) {
            ForEach(nodes) { node in
                TopologyNodeView(node: node)
            }
        }
        .frame(width: width)
        .accessibilityIdentifier("topology.column.\(side.rawValue)")
    }

    private var leftNodes: [TopologyNode] {
        let clients = model.dashboard?.executionClients
        return [
            TopologyNode(
                id: "codex-app",
                title: "Codex 应用",
                compactTitle: "Codex",
                detail: endpointDetail(clients?.codexApp),
                symbol: "chevron.left.forwardslash.chevron.right",
                tint: endpointTint(clients?.codexApp)
            ),
            TopologyNode(
                id: "vscode",
                title: "VS Code",
                compactTitle: "VS Code",
                detail: sessionEndpointDetail(clients?.vscode),
                symbol: "chevron.left.forwardslash.chevron.right",
                tint: sessionEndpointTint(clients?.vscode)
            ),
            TopologyNode(
                id: "cli",
                title: "命令行（CLI）",
                compactTitle: "命令行",
                detail: sessionEndpointDetail(clients?.cli),
                symbol: "terminal",
                tint: sessionEndpointTint(clients?.cli)
            ),
        ]
    }

    private var rightNodes: [TopologyNode] {
        let channels = model.dashboard?.messageChannels
        let legacy = channels?.legacyUnattributed.accountCount ?? 0 > 0
        return [
            channelNode("telegram", "Telegram", channels?.telegram, "paperplane", legacy: legacy),
            channelNode("feishu", "飞书", channels?.feishu, "bubble.left.and.text.bubble.right", legacy: legacy),
            channelNode("wechat", "微信", channels?.wechat, "message", legacy: legacy),
            channelNode("wecom", "企业微信", channels?.wecom, "person.2", legacy: legacy),
        ]
    }

    private func channelNode(
        _ id: String,
        _ title: String,
        _ channel: ManageDashboard.MessageChannel?,
        _ symbol: String,
        legacy: Bool
    ) -> TopologyNode {
        if legacy {
            return TopologyNode(
                id: id,
                title: title,
                compactTitle: title,
                detail: "兼容模式",
                symbol: symbol,
                tint: .caution
            )
        }
        return TopologyNode(
            id: id,
            title: title,
            compactTitle: title,
            detail: channelDetail(channel),
            symbol: symbol,
            tint: channelTint(channel)
        )
    }

    private var summary: String {
        let connectedClients = leftNodes.filter { $0.tint == .positive }.count
        let connectedChannels = rightNodes.filter { $0.tint == .positive }.count
        return "\(connectedClients) 客户端 · \(connectedChannels) 渠道在线"
    }

    private var remoteTint: StatusTint {
        guard let dashboard = model.dashboard else { return .secondary }
        return dashboard.remoteControlHealthy ? .positive : dashboard.remoteControlConnected ? .caution : .negative
    }

    private var bridgeTint: StatusTint {
        guard let bridgeRunning = model.dashboard?.bridgeRunning else { return .secondary }
        return bridgeRunning ? .positive : .caution
    }

    private var unavailableDetail: String {
        switch model.dashboardState {
        case .legacy: "需要更新"
        case .unauthorized: "需要授权"
        case .unavailable, .offline: "不可用"
        case .stale: "上次状态"
        case .starting: "正在启动"
        default: "检查中"
        }
    }

    private func endpointDetail(_ endpoint: ManageDashboard.Endpoint?) -> String {
        guard let endpoint else { return unavailableDetail }
        if endpoint.connected { return "已连接" }
        return endpoint.configured ? "可用" : "未检测到"
    }

    private func endpointTint(_ endpoint: ManageDashboard.Endpoint?) -> StatusTint {
        guard let endpoint else { return .secondary }
        if endpoint.connected { return .positive }
        return endpoint.configured ? .caution : .secondary
    }

    private func sessionEndpointDetail(_ endpoint: ManageDashboard.Endpoint?) -> String {
        guard let endpoint else { return unavailableDetail }
        return endpoint.connected ? "已连接" : "无活跃会话"
    }

    private func sessionEndpointTint(_ endpoint: ManageDashboard.Endpoint?) -> StatusTint {
        guard let endpoint else { return .secondary }
        return endpoint.connected ? .positive : .secondary
    }

    private func channelDetail(_ channel: ManageDashboard.MessageChannel?) -> String {
        guard let channel else { return unavailableDetail }
        guard channel.accountCount > 0 else { return "未配置" }
        return "已连接 \(channel.connectedAccountCount)/\(channel.accountCount)"
    }

    private func channelTint(_ channel: ManageDashboard.MessageChannel?) -> StatusTint {
        guard let channel else { return .secondary }
        guard channel.accountCount > 0 else { return .secondary }
        return channel.accountCount == channel.connectedAccountCount ? .positive : .caution
    }
}

private enum TopologyNodeSide: String {
    case left
    case right
}

private struct TopologyNode: Identifiable {
    let id: String
    let title: String
    let compactTitle: String
    let detail: String
    let symbol: String
    let tint: StatusTint
}

private struct TopologyNodeView: View {
    let node: TopologyNode

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: node.symbol)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(node.tint.color)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                ViewThatFits(in: .horizontal) {
                    Text(node.title)
                        .fixedSize(horizontal: true, vertical: false)
                    Text(node.compactTitle)
                }
                .font(.subheadline.weight(.semibold))
                .lineLimit(1)
                Text(node.detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            Circle()
                .fill(node.tint.color)
                .frame(width: 7, height: 7)
        }
        .padding(.horizontal, 11)
        .frame(maxWidth: .infinity, minHeight: 56, maxHeight: 56)
        .topologyNodeSurface(cornerRadius: ThreadRelayRadius.content)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("topology.node.\(node.id)")
    }
}

private struct TopologyServiceNode: View {
    let status: ServiceStatus
    let remoteTint: StatusTint
    let bridgeTint: StatusTint

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "server.rack")
                    .font(.system(size: 21, weight: .semibold))
                    .foregroundStyle(status.tint.color)
                VStack(alignment: .leading, spacing: 2) {
                    Text("ThreadRelay")
                        .font(.headline.weight(.semibold))
                    Text("本地桥接服务")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Divider()
            HStack(spacing: 8) {
                TopologyStatusMark(label: "远程控制", tint: remoteTint)
                TopologyStatusMark(label: "消息桥接", tint: bridgeTint)
            }
        }
        .padding(14)
        .frame(minHeight: 118, maxHeight: 118, alignment: .leading)
        .topologyNodeSurface(cornerRadius: ThreadRelayRadius.overlay)
    }
}

private struct TopologyStatusMark: View {
    let label: String
    let tint: StatusTint

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(tint.color)
                .frame(width: 6, height: 6)
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }
}

private struct TopologyConnectorCanvas: View {
    let size: CGSize
    let leftCount: Int
    let rightCount: Int
    let nodeHeight: CGFloat
    let nodeSpacing: CGFloat
    let sideWidth: CGFloat
    let serviceWidth: CGFloat
    let gap: CGFloat
    let layoutPadding: CGFloat
    let leftTint: StatusTint
    let rightTint: StatusTint
    let leftBranchTints: [StatusTint]
    let rightBranchTints: [StatusTint]

    var body: some View {
        Canvas { context, _ in
            let leftCenters = centers(count: leftCount)
            let rightCenters = centers(count: rightCount)
            let serviceLeft = layoutPadding + sideWidth + gap
            let serviceRight = serviceLeft + serviceWidth
            let leftNodeEdge = layoutPadding + sideWidth
            let rightNodeEdge = serviceRight + gap
            let leftTrunk = leftNodeEdge + 8
            let rightTrunk = serviceRight + gap - 8
            let middleY = size.height / 2

            drawBranches(
                context: &context,
                centers: leftCenters,
                nodeEdge: leftNodeEdge,
                trunk: leftTrunk,
                branchTints: leftBranchTints
            )
            drawTrunk(
                context: &context,
                x: leftTrunk,
                centers: leftCenters,
                tint: leftTint
            )
            drawMain(
                context: &context,
                from: CGPoint(x: leftTrunk, y: middleY),
                to: CGPoint(x: serviceLeft, y: middleY),
                tint: leftTint
            )

            drawBranches(
                context: &context,
                centers: rightCenters,
                nodeEdge: rightNodeEdge,
                trunk: rightTrunk,
                branchTints: rightBranchTints
            )
            drawTrunk(
                context: &context,
                x: rightTrunk,
                centers: rightCenters,
                tint: rightTint
            )
            drawMain(
                context: &context,
                from: CGPoint(x: serviceRight, y: middleY),
                to: CGPoint(x: rightTrunk, y: middleY),
                tint: rightTint
            )
        }
        .allowsHitTesting(false)
    }

    private func centers(count: Int) -> [CGFloat] {
        let total = CGFloat(count) * nodeHeight + CGFloat(max(0, count - 1)) * nodeSpacing
        let top = max(0, (size.height - total) / 2)
        return (0..<count).map { top + CGFloat($0) * (nodeHeight + nodeSpacing) + nodeHeight / 2 }
    }

    private func drawBranches(
        context: inout GraphicsContext,
        centers: [CGFloat],
        nodeEdge: CGFloat,
        trunk: CGFloat,
        branchTints: [StatusTint]
    ) {
        for (index, center) in centers.enumerated() {
            drawMain(
                context: &context,
                from: CGPoint(x: nodeEdge, y: center),
                to: CGPoint(x: trunk, y: center),
                tint: branchTints.indices.contains(index) ? branchTints[index] : .secondary
            )
        }
    }

    private func drawTrunk(
        context: inout GraphicsContext,
        x: CGFloat,
        centers: [CGFloat],
        tint: StatusTint
    ) {
        guard let first = centers.first, let last = centers.last else { return }
        drawMain(
            context: &context,
            from: CGPoint(x: x, y: first),
            to: CGPoint(x: x, y: last),
            tint: tint
        )
    }

    private func drawMain(
        context: inout GraphicsContext,
        from: CGPoint,
        to: CGPoint,
        tint: StatusTint
    ) {
        var path = Path()
        path.move(to: from)
        path.addLine(to: to)
        context.stroke(path, with: .color(tint.color.opacity(0.82)), lineWidth: 1.5)
    }
}

private extension StatusTint {
    var color: Color {
        switch self {
        case .secondary: .secondary
        case .positive: .green
        case .caution: .orange
        case .negative: .red
        }
    }
}

private extension View {
    @ViewBuilder
    func topologyNodeSurface(cornerRadius: CGFloat) -> some View {
        if #available(macOS 26.0, *) {
            glassEffect(.regular, in: RoundedRectangle(cornerRadius: cornerRadius))
        } else {
            background(.regularMaterial, in: RoundedRectangle(cornerRadius: cornerRadius))
                .overlay {
                    RoundedRectangle(cornerRadius: cornerRadius)
                        .strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5)
                }
        }
    }
}
