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

struct FloatingControlSurface<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        Group {
            if #available(macOS 26, *) {
                content
                    .padding(18)
                    .glassEffect(.regular, in: RoundedRectangle(cornerRadius: ThreadRelayRadius.overlay))
                    .glassEffectTransition(.materialize)
            } else {
                content
                    .padding(18)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: ThreadRelayRadius.overlay))
            }
        }
    }
}
