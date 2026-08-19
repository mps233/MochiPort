import AppKit
import SwiftUI

enum ThreadRelaySpacing {
    static let compact: CGFloat = 8
    static let standard: CGFloat = 12
    static let section: CGFloat = 24
    static let page: CGFloat = 28
}

enum ThreadRelayRadius {
    static let content: CGFloat = 12
    static let overlay: CGFloat = 16
}

/// Shared geometry for pages hosted in the main detail column. The native
/// navigation title identifies the page; these values keep the content area
/// predictable as users move between sections.
enum ThreadRelayPageLayout {
    static let maxContentWidth: CGFloat = 960
    static let horizontalPadding: CGFloat = 28
    static let topPadding: CGFloat = 20
    static let bottomPadding: CGFloat = 28
    static let sectionSpacing: CGFloat = 24
}

/// Neutral grouped surface matching the body of a native grouped `Form`.
/// Glass remains reserved for floating chrome such as the quota dock.
struct SettingsGroupSurface<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            content
        }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .settingsGroupedBackground()
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

/// AppKit owns the complete search-field geometry, appearance and interaction.
struct NativeSearchField: NSViewRepresentable {
    @Binding var text: String
    let prompt: String

    init(_ prompt: String, text: Binding<String>) {
        self.prompt = prompt
        _text = text
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.controlSize = .large
        field.placeholderString = prompt
        field.sendsSearchStringImmediately = true
        field.sendsWholeSearchString = false
        field.delegate = context.coordinator
        field.setAccessibilityLabel(prompt)
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        context.coordinator.text = $text
        if field.stringValue != text {
            field.stringValue = text
        }
        if field.placeholderString != prompt {
            field.placeholderString = prompt
            field.setAccessibilityLabel(prompt)
        }
    }

    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var text: Binding<String>

        init(text: Binding<String>) {
            self.text = text
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSSearchField else { return }
            text.wrappedValue = field.stringValue
        }
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
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            dismiss()
        }
    }
}
