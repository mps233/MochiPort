import SwiftUI

enum MochiPortSpacing {
    static let compact: CGFloat = 8
    static let standard: CGFloat = 12
    static let section: CGFloat = 24
    static let page: CGFloat = 28
}

enum MochiPortRadius {
    static let content: CGFloat = 12
    static let overlay: CGFloat = 16
}

/// Shared geometry for pages hosted in the main detail column. The native
/// navigation title identifies the page; these values keep the content area
/// predictable as users move between sections.
enum MochiPortPageLayout {
    static let maxContentWidth: CGFloat = 960
    static let horizontalPadding: CGFloat = 28
    static let topPadding: CGFloat = 20
    static let bottomPadding: CGFloat = 28
    static let sectionSpacing: CGFloat = 24
}

protocol MochiPortSegmentItem: CaseIterable, Equatable, Identifiable {
    var title: String { get }
    var symbol: String { get }
}

extension StatusTint {
    var color: Color {
        switch self {
        case .secondary: .secondary
        case .positive: .green
        case .caution: .orange
        case .negative: .red
        }
    }
}

struct GlassSegmentedControl<Item: MochiPortSegmentItem>: View {
    @Binding private var selection: Item
    let accessibilityLabel: String
    let help: (Item) -> String
    @Namespace private var selectionNamespace

    init(
        selection: Binding<Item>,
        accessibilityLabel: String,
        help: @escaping (Item) -> String
    ) {
        _selection = selection
        self.accessibilityLabel = accessibilityLabel
        self.help = help
    }

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Array(Item.allCases)) { item in
                segment(item)
            }
        }
        .padding(3)
        .glassEffect(.regular.interactive(), in: .capsule)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityLabel)
    }

    private func segment(_ item: Item) -> some View {
        let isSelected = selection == item

        return Button {
            guard selection != item else { return }
            withAnimation(.easeInOut(duration: 0.2)) {
                selection = item
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: item.symbol)
                    .font(.system(size: 10, weight: .semibold))
                Text(item.title)
                    .font(.system(size: 11, weight: .semibold))
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 5)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(isSelected ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
        .background {
            if isSelected {
                Capsule()
                    .fill(Color.accentColor)
                    .matchedGeometryEffect(id: "selected", in: selectionNamespace)
            }
        }
        .accessibilityLabel(item.title)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
        .help(help(item))
    }
}

private struct SettingsGroupedBackgroundModifier: ViewModifier {
    func body(content: Content) -> some View {
        content.background {
            SettingsGroupedSurfaceBackground()
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
    }
}

/// Shared fill for ordinary content surfaces. Its values match the neutral
/// row surface used by the native grouped forms in Codex access and Settings.
struct SettingsGroupedSurfaceBackground: View {
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Color(
            .sRGB,
            white: colorScheme == .dark ? 47.0 / 255.0 : 1,
            opacity: 1
        )
    }
}

extension View {
    func settingsGroupedBackground() -> some View {
        modifier(SettingsGroupedBackgroundModifier())
    }
}

/// Shared success capsule shown at the bottom of the detail column after a
/// management action completes. Auto-dismisses after three seconds; the timer
/// restarts whenever a new feedback message replaces the current one.
struct ActionFeedbackCapsule: View {
    let feedback: ActionFeedback
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Label(feedback.message, systemImage: "checkmark.circle.fill")
            Button {
                dismiss()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("关闭提示")
            .accessibilityLabel("关闭提示")
        }
        .font(.callout)
        .foregroundStyle(.green)
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .background(.regularMaterial, in: Capsule())
        .overlay {
            Capsule().stroke(Color.green.opacity(0.25), lineWidth: 1)
        }
        .accessibilityIdentifier("feedback.capsule")
        .task(id: feedback.id) {
            let duration: Duration = feedback.message.contains("\n") ? .seconds(8) : .seconds(3)
            try? await Task.sleep(for: duration)
            guard !Task.isCancelled else { return }
            dismiss()
        }
    }
}
