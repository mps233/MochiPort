import AppKit
import SwiftUI

/// A compact, deterministic map of the clients and message channels attached
/// to MochiPort. This intentionally behaves like a bridge diagram rather
/// than a free-form service graph: there are only two endpoint rails and one
/// local service in the middle.
/// 各拓扑节点真实渲染中心的锚点——连线画布据此绘图，保证与节点位置严格一致。
private struct TopologyAnchorKey: PreferenceKey {
    static var defaultValue: [String: Anchor<CGPoint>] { [:] }
    static func reduce(value: inout [String: Anchor<CGPoint>], nextValue: () -> [String: Anchor<CGPoint>]) {
        value.merge(nextValue()) { current, _ in current }
    }
}

struct ConnectionTopologyView: View {
    @EnvironmentObject private var model: AppModel
    @State private var nodePoints: [String: CGPoint] = [:]

    private let nodeHeight: CGFloat = 52
    private let compactNodeHeight: CGFloat = 28
    private let nodeSpacing: CGFloat = 10
    private let topologyVerticalPadding: CGFloat = 12

    /// 图高 = 最高的一列 + 上下呼吸空间；不再固定 286pt 留出大片空带。
    private var topologyHeight: CGFloat {
        let leftStack = leftNodes.reduce(0) { $0 + ($1.isCompact ? compactNodeHeight : nodeHeight) }
        let rightStack = rightNodes.reduce(0) { $0 + ($1.isCompact ? compactNodeHeight : nodeHeight) }
        return max(leftStack, rightStack) + topologyVerticalPadding * 2
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("连接拓扑")
                        .font(.headline)
                    Text("客户端和消息渠道通过本地服务的实时连接")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                Text(summary)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            GeometryReader { proxy in
                topologyLayout(in: proxy.size)
                    .onPreferenceChange(TopologyAnchorKey.self) { anchors in
                        var points: [String: CGPoint] = [:]
                        for (id, anchor) in anchors { points[id] = proxy[anchor] }
                        if points != nodePoints { nodePoints = points }
                    }
            }
            .frame(minHeight: topologyHeight, idealHeight: topologyHeight, maxHeight: topologyHeight)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overview.connection-topology")
    }

    @ViewBuilder
    private func topologyLayout(in size: CGSize) -> some View {
        let metrics = TopologyLayoutMetrics(size: size)
        topologyStack(
            size: CGSize(width: metrics.layoutWidth, height: size.height),
            metrics: metrics,
            points: nodePoints
        )
        .frame(width: metrics.layoutWidth, height: size.height)
        .frame(maxWidth: .infinity)
    }

    private func topologyStack(
        size: CGSize,
        metrics: TopologyLayoutMetrics,
        points: [String: CGPoint]
    ) -> some View {
        ZStack {
            TopologyConnectorCanvas(
                size: size,
                points: points,
                leftNodes: leftNodes,
                rightNodes: rightNodes,
                sideWidth: metrics.sideWidth,
                serviceWidth: metrics.serviceWidth
            )

            HStack(alignment: .center, spacing: metrics.gap) {
                nodeColumn(leftNodes, width: metrics.sideWidth, side: .left)
                TopologyServiceNode(
                    status: model.serviceStatus,
                    remoteTint: remoteTint,
                    bridgeTint: bridgeTint
                )
                .frame(width: metrics.serviceWidth)
                .anchorPreference(key: TopologyAnchorKey.self, value: .center) {
                    ["service": $0]
                }
                .accessibilityIdentifier("topology.node.local-service")
                nodeColumn(rightNodes, width: metrics.sideWidth, side: .right)
            }
            .padding(.horizontal, metrics.layoutPadding)
            .frame(
                width: size.width,
                height: size.height,
                alignment: .leading
            )
        }
        .frame(width: size.width, height: size.height)
    }

    private func nodeColumn(
        _ nodes: [TopologyNode],
        width: CGFloat,
        side: TopologyNodeSide
    ) -> some View {
        VStack(alignment: .leading, spacing: nodeSpacing) {
            ForEach(nodes) { node in
                TopologyNodeView(node: node)
                    .anchorPreference(key: TopologyAnchorKey.self, value: .center) {
                        [node.id: $0]
                    }
            }
        }
        .frame(width: width, alignment: .top)
        .accessibilityIdentifier("topology.column.\(side.rawValue)")
    }

    private var leftNodes: [TopologyNode] {
        let clients = model.dashboard?.executionClients
        return [
            TopologyNode(
                id: "codex-app",
                title: "Codex 远程控制",
                compactTitle: "Codex 远程控制",
                detail: endpointDetail(clients?.codexApp),
                symbol: "chevron.left.forwardslash.chevron.right",
                tint: endpointTint(clients?.codexApp),
                accounts: [],
                logo: .codex
            ),
            TopologyNode(
                id: "vscode",
                title: "Codex for VSCode",
                compactTitle: "Codex VSCode",
                detail: sessionEndpointDetail(clients?.vscode),
                symbol: "chevron.left.forwardslash.chevron.right",
                tint: sessionEndpointTint(clients?.vscode),
                accounts: [],
                logo: .vscode
            ),
            TopologyNode(
                id: "cli",
                title: "Codex CLI",
                compactTitle: "Codex CLI",
                detail: sessionEndpointDetail(clients?.cli),
                symbol: "terminal",
                tint: sessionEndpointTint(clients?.cli),
                accounts: [],
                logo: .codexCLI
            ),
        ]
    }

    private var rightNodes: [TopologyNode] {
        let channels = model.dashboard?.messageChannels
        let legacy = channels?.legacyUnattributed.accountCount ?? 0 > 0
        var nodes = [
            channelNode("telegram", "Telegram", channels?.telegram, "paperplane", legacy: legacy),
            channelNode("feishu", "飞书", channels?.feishu, "bubble.left.and.text.bubble.right", legacy: legacy),
            channelNode("wechat", "微信", channels?.wechat, "message", legacy: legacy),
            channelNode("wecom", "企业微信", channels?.wecom, "person.2", legacy: legacy),
        ]

        // 未配置的渠道折叠成一行紧凑节点：它们不携带实时状态，
        // 不应该和活的连接等大等重地占用版面。
        let unconfiguredTitles = nodes
            .filter { $0.tint == .secondary }
            .map(\.title)
        if !unconfiguredTitles.isEmpty {
            let joined = unconfiguredTitles.joined(separator: " · ")
            nodes.removeAll { $0.tint == .secondary }
            nodes.append(
                TopologyNode(
                    id: "unconfigured-channels",
                    title: joined,
                    compactTitle: joined,
                    detail: "未配置",
                    symbol: "circle.dashed",
                    tint: .secondary,
                    accounts: [],
                    logo: nil,
                    isCompact: true
                )
            )
        }
        return nodes
    }

    private func channelNode(
        _ id: String,
        _ title: String,
        _ channel: ManageDashboard.MessageChannel?,
        _ symbol: String,
        legacy: Bool
    ) -> TopologyNode {
        let accounts = model.imAccounts
            .compactMap(MessagingAccountSummary.init)
            .filter { $0.platform.rawValue == id }
            .sorted { lhs, rhs in
                if lhs.connected != rhs.connected { return lhs.connected && !rhs.connected }
                let lhsName = lhs.displayName ?? lhs.accountID
                let rhsName = rhs.displayName ?? rhs.accountID
                return lhsName.localizedCaseInsensitiveCompare(rhsName) == .orderedAscending
            }

        if legacy {
            return TopologyNode(
                id: id,
                title: title,
                compactTitle: title,
                detail: "兼容模式",
                symbol: symbol,
                tint: .caution,
                accounts: accounts,
                logo: nil
            )
        }
        return TopologyNode(
            id: id,
            title: title,
            compactTitle: title,
            detail: channelDetail(channel),
            symbol: symbol,
            tint: channelTint(channel),
            accounts: accounts,
            logo: nil
        )
    }

    private var summary: String {
        let connectedClients = leftNodes.count(where: { $0.tint == .positive })
        let connectedChannels = rightNodes.count(where: { $0.tint == .positive })
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

private struct TopologyLayoutMetrics {
    let layoutWidth: CGFloat
    let sideWidth: CGFloat
    let serviceWidth: CGFloat
    let gap: CGFloat
    let layoutPadding: CGFloat = 8

    init(size: CGSize) {
        layoutWidth = min(size.width, 720)
        let contentWidth = max(0, layoutWidth - 16)
        let columnGap = min(24, max(10, contentWidth * 0.035))
        let columnsWidth = max(0, contentWidth - columnGap * 2)
        let preferredServiceWidth = min(202, max(120, columnsWidth * 0.30))
        let availableSideWidth = max(0, (columnsWidth - preferredServiceWidth) / 2)

        // Keep the rails readable while allowing the whole diagram to shrink
        // as one unit instead of letting fixed minimums push columns out of
        // the window at smaller widths.
        if availableSideWidth >= 84 {
            sideWidth = min(184, availableSideWidth)
            serviceWidth = preferredServiceWidth
            gap = columnGap + max(0, (availableSideWidth - sideWidth) / 2)
        } else {
            let compactServiceWidth = min(preferredServiceWidth, max(96, columnsWidth * 0.34))
            sideWidth = max(72, (columnsWidth - compactServiceWidth) / 2)
            serviceWidth = max(96, columnsWidth - sideWidth * 2)
            gap = columnGap
        }
    }
}

private enum ClientLogoKind: String, Hashable {
    case codex
    case vscode
    case codexCLI = "codex-cli"
}

/// Loads the local client brand marks used by the topology. The SF Symbol
/// remains a deliberate fallback so the topology stays legible if a resource
/// is unavailable in an older bundle.
@MainActor
private enum ClientLogoStore {
    private static var cache: [ClientLogoKind: NSImage?] = [:]

    static func image(for kind: ClientLogoKind) -> NSImage? {
        if let cached = cache[kind] {
            return cached
        }

        let loaded = loadImage(named: kind.rawValue)
        cache[kind] = loaded
        return loaded
    }

    private static func loadImage(named name: String) -> NSImage? {
        #if SWIFT_PACKAGE
        let bundle = Bundle.module
        #else
        let bundle = Bundle.main
        #endif
        guard let url = bundle.url(
            forResource: name,
            withExtension: "svg",
            subdirectory: "ClientLogos"
        ) else {
            return nil
        }
        return NSImage(contentsOf: url)
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
    let accounts: [MessagingAccountSummary]
    let logo: ClientLogoKind?
    var isCompact = false
}

private struct TopologyNodeView: View {
    let node: TopologyNode
    @State private var isHovering = false

    var body: some View {
        if node.isCompact {
            compactRow
        } else {
            fullNode
        }
    }

    /// 未配置渠道的紧凑行：占位但不与活节点争夺视觉重量。
    private var compactRow: some View {
        HStack(spacing: 7) {
            Image(systemName: node.symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.tertiary)
                .frame(width: 20)
            Text(node.title)
                .font(.caption)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
            Spacer(minLength: 6)
            Text(node.detail)
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 11)
        .frame(maxWidth: .infinity, minHeight: 28, maxHeight: 28)
        .background(
            RoundedRectangle(cornerRadius: MochiPortRadius.content, style: .continuous)
                .fill(Color.primary.opacity(0.015))
        )
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("topology.node.\(node.id)")
    }

    private var fullNode: some View {
        let isIdle = node.tint == .secondary
        return HStack(spacing: 9) {
            // Both endpoint rails use the same reading order.  The channel
            // avatar belongs beside its name, while the status dot stays at
            // the trailing edge as a compact state cue.
            TopologyNodeIcon(node: node)

            VStack(alignment: .leading, spacing: 2) {
                ViewThatFits(in: .horizontal) {
                    Text(node.title)
                        .fixedSize(horizontal: true, vertical: false)
                    Text(node.compactTitle)
                }
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(isIdle ? Color.secondary : Color.primary)
                .minimumScaleFactor(0.84)
                .lineLimit(1)
                Text(node.detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .minimumScaleFactor(0.84)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            statusMark
        }
        .padding(.horizontal, 11)
        .frame(maxWidth: .infinity, minHeight: 52, maxHeight: 52)
        .topologyEndpointSurface(tint: node.tint, isHovering: isHovering)
        .contentShape(RoundedRectangle(cornerRadius: MochiPortRadius.content, style: .continuous))
        .onHover { isHovering = $0 }
        .animation(.easeOut(duration: 0.16), value: isHovering)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("topology.node.\(node.id)")
    }

    private var statusMark: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(node.tint.color)
                .frame(width: 6, height: 6)
        }
        .frame(width: 15, alignment: .trailing)
    }
}

private struct TopologyNodeIcon: View {
    let node: TopologyNode

    var body: some View {
        if node.accounts.isEmpty {
            if let logo = node.logo,
               let image = ClientLogoStore.image(for: logo) {
                ZStack {
                    Circle()
                        .fill(Color.white.opacity(0.94))
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFit()
                        .padding(3)
                }
                .frame(width: 28, height: 28)
                .clipShape(Circle())
                .overlay {
                    Circle()
                        .strokeBorder(Color.white.opacity(0.52), lineWidth: 0.6)
                }
            } else {
                Image(systemName: node.symbol)
                    .font(.system(size: 14, weight: .semibold))
                    .symbolRenderingMode(.hierarchical)
                    .foregroundStyle(node.tint.color)
                    .frame(width: 28, height: 28)
            }
        } else {
            TopologyAvatarStack(accounts: node.accounts)
        }
    }
}

private struct TopologyAvatarStack: View {
    let accounts: [MessagingAccountSummary]
    private let visibleLimit = 3
    private let avatarSize: CGFloat = 27
    private let overlap: CGFloat = 9

    private var visibleAccounts: ArraySlice<MessagingAccountSummary> {
        accounts.prefix(visibleLimit)
    }

    var body: some View {
        HStack(spacing: -overlap) {
            ForEach(Array(visibleAccounts)) { account in
                MessagingAccountAvatar(account: account, size: avatarSize)
                    .opacity(account.connected ? 1 : 0.46)
                    .zIndex(account.connected ? 1 : 0)
            }
            if accounts.count > visibleLimit {
                Text("+\(accounts.count - visibleLimit)")
                    .font(.caption2.monospacedDigit().weight(.semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: avatarSize, height: avatarSize)
                    .background(.quaternary, in: Circle())
                    .overlay { Circle().strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5) }
                    .zIndex(2)
            }
        }
        .frame(width: avatarWidth, height: avatarSize, alignment: .leading)
        .accessibilityHidden(true)
    }

    private var avatarWidth: CGFloat {
        let visibleCount = min(accounts.count, visibleLimit)
        let count = accounts.count > visibleLimit ? visibleCount + 1 : visibleCount
        return avatarSize + CGFloat(max(0, count - 1)) * (avatarSize - overlap)
    }
}

private struct TopologyServiceNode: View {
    let status: ServiceStatus
    let remoteTint: StatusTint
    let bridgeTint: StatusTint

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                ZStack {
                    Circle()
                        .fill(status.tint.color.opacity(0.12))
                    Image(systemName: "server.rack")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(status.tint.color)
                }
                .frame(width: 35, height: 35)

                VStack(alignment: .leading, spacing: 2) {
                    Text("MochiPort")
                        .font(.headline.weight(.semibold))
                    Text("本地桥接服务")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 4)
            }

            HStack(spacing: 6) {
                TopologyServiceStatus(title: status.title, tint: status.tint)
                Spacer(minLength: 2)
                TopologyStatusMark(label: "远程控制", tint: remoteTint)
                TopologyStatusMark(label: "消息桥接", tint: bridgeTint)
            }
        }
        .padding(14)
        .frame(minHeight: 118, maxHeight: 118, alignment: .leading)
        .topologyServiceSurface(tint: status.tint)
    }
}

private struct TopologyServiceStatus: View {
    let title: String
    let tint: StatusTint

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(tint.color)
                .frame(width: 6, height: 6)
            Text(title)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
        }
    }
}

private struct TopologyStatusMark: View {
    let label: String
    let tint: StatusTint

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(tint.color)
                .frame(width: 5, height: 5)
            Text(label)
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
    }
}

private struct TopologyConnectorCanvas: View {
    let size: CGSize
    /// 节点 id → 真实渲染中心；由 anchorPreference + GeometryProxy 解析，永远与节点一致。
    let points: [String: CGPoint]
    let leftNodes: [TopologyNode]
    let rightNodes: [TopologyNode]
    let sideWidth: CGFloat
    let serviceWidth: CGFloat

    var body: some View {
        Canvas { context, _ in
            guard let serviceCenter = points["service"] else { return }
            let serviceLeft = serviceCenter.x - serviceWidth / 2
            let serviceRight = serviceCenter.x + serviceWidth / 2

            for activePass in [false, true] {
                for node in leftNodes {
                    guard let center = points[node.id] else { continue }
                    let tint = node.tint
                    guard (tint != .secondary) == activePass else { continue }
                    drawLink(
                        context: &context,
                        from: CGPoint(x: center.x + sideWidth / 2, y: center.y),
                        to: CGPoint(x: serviceLeft, y: serviceCenter.y),
                        tint: tint
                    )
                }
                for node in rightNodes {
                    guard let center = points[node.id] else { continue }
                    let tint = node.tint
                    guard (tint != .secondary) == activePass else { continue }
                    drawLink(
                        context: &context,
                        from: CGPoint(x: serviceRight, y: serviceCenter.y),
                        to: CGPoint(x: center.x - sideWidth / 2, y: center.y),
                        tint: tint
                    )
                }
            }
        }
        .allowsHitTesting(false)
    }

    /// 平滑 S 曲线连接：消息是双向的，因此不画方向箭头，
    /// 端口圆点与节点自身状态承担状态表达。
    private func drawLink(
        context: inout GraphicsContext,
        from: CGPoint,
        to: CGPoint,
        tint: StatusTint
    ) {
        let midX = (from.x + to.x) / 2
        var path = Path()
        path.move(to: from)
        path.addCurve(
            to: to,
            control1: CGPoint(x: midX, y: from.y),
            control2: CGPoint(x: midX, y: to.y)
        )

        guard tint != .secondary else {
            context.stroke(
                path,
                with: .color(Color.secondary.opacity(0.26)),
                style: StrokeStyle(lineWidth: 1, lineCap: .round, lineJoin: .round, dash: [2, 4])
            )
            return
        }

        context.drawLayer { layer in
            layer.addFilter(.blur(radius: 2.2))
            layer.stroke(
                path,
                with: .color(tint.color.opacity(0.18)),
                style: StrokeStyle(lineWidth: 3.8, lineCap: .round, lineJoin: .round)
            )
        }
        context.stroke(
            path,
            with: .color(tint.color.opacity(0.72)),
            style: StrokeStyle(lineWidth: 1.35, lineCap: .round, lineJoin: .round)
        )

        for point in [from, to] {
            let port = CGRect(x: point.x - 2.2, y: point.y - 2.2, width: 4.4, height: 4.4)
            context.fill(Path(ellipseIn: port), with: .color(tint.color.opacity(0.82)))
        }
    }
}

private extension View {
    @ViewBuilder
    func topologyEndpointSurface(tint _: StatusTint, isHovering: Bool) -> some View {
        let shape = RoundedRectangle(
            cornerRadius: MochiPortRadius.content,
            style: .continuous
        )
        let fillOpacity = isHovering ? 0.05 : 0.025

        self
            .background {
                shape
                    .fill(Color.primary.opacity(fillOpacity))
            }
            .overlay {
                shape.strokeBorder(
                    Color.primary.opacity(isHovering ? 0.16 : 0.075),
                    lineWidth: 0.5
                )
            }
            .shadow(
                color: Color.black.opacity(isHovering ? 0.07 : 0.025),
                radius: isHovering ? 8 : 3,
                y: isHovering ? 3 : 1
            )
    }

    @ViewBuilder
    func topologyServiceSurface(tint _: StatusTint) -> some View {
        let shape = RoundedRectangle(
            cornerRadius: MochiPortRadius.overlay,
            style: .continuous
        )

        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: shape)
        } else {
            self
                .background {
                    shape.fill(.regularMaterial)
                }
                .overlay {
                    shape.strokeBorder(
                        Color.primary.opacity(0.09),
                        lineWidth: 0.5
                    )
                }
        }
    }
}
