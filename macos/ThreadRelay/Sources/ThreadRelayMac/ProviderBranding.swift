import AppKit
import SwiftUI

/// Brand mark shown next to an AI Gateway provider. The mapping mirrors the
/// Shared provider logo mapping used by the management clients.
enum ProviderLogoKind: String {
    case openAI = "openai"
    case deepSeek = "deepseek"
    case grok = "grok"
    case anthropic = "anthropic"
    case zhipu = "zhipu"

    /// Brand accent used by the monogram fallback when the SVG asset cannot
    /// be loaded.
    var fallbackColor: Color {
        switch self {
        case .openAI: Color(red: 0.06, green: 0.64, blue: 0.5)
        case .deepSeek: Color(red: 0.3, green: 0.42, blue: 1.0)
        case .grok: Color(red: 0.35, green: 0.35, blue: 0.38)
        case .anthropic: Color(red: 0.85, green: 0.47, blue: 0.34)
        case .zhipu: Color(red: 0.22, green: 0.35, blue: 1.0)
        }
    }
}

func providerLogoKind(providerType: String, compatibility: String?) -> ProviderLogoKind? {
    switch providerType {
    case "open_ai_responses": return .openAI
    case "deepseek_responses": return .deepSeek
    case "grok_responses": return .grok
    case "chat_completions": return .deepSeek
    case "anthropic_messages":
        if compatibility == "glm_anthropic" || compatibility == "zhipu_anthropic" {
            return .zhipu
        }
        return .anthropic
    default: return nil
    }
}

/// Loads the bundled monochrome brand SVGs once and re-tints them through
/// the template rendering pipeline so they follow light and dark appearance.
@MainActor
enum ProviderLogoStore {
    private static var cache: [ProviderLogoKind: NSImage?] = [:]

    static func image(for kind: ProviderLogoKind) -> NSImage? {
        if let cached = cache[kind] {
            return cached
        }
        let loaded = loadImage(named: kind.rawValue)
        cache[kind] = loaded
        return loaded
    }

    private static func loadImage(named name: String) -> NSImage? {
        guard let url = logoURL(named: name),
              let image = NSImage(contentsOf: url)
        else {
            return nil
        }
        // Template rendering uses only the alpha channel, so the single
        // baked-in fill color re-tints with the current foreground style.
        image.isTemplate = true
        return image
    }

    private static func logoURL(named name: String) -> URL? {
        #if SWIFT_PACKAGE
        let bundle = Bundle.module
        #else
        let bundle = Bundle.main
        #endif
        return bundle.url(
            forResource: name,
            withExtension: "svg",
            subdirectory: "ProviderLogos"
        )
    }
}

/// Brand icon for a provider row. Falls back to a circled monogram of the
/// provider name in the brand color when the SVG asset is unavailable, and
/// to a generic server glyph when the provider type is unknown.
struct ProviderLogoView: View {
    let providerType: String
    let compatibility: String?
    let providerName: String
    var size: CGFloat = 20

    var body: some View {
        Group {
            if let kind = providerLogoKind(
                providerType: providerType,
                compatibility: compatibility
            ) {
                if let image = ProviderLogoStore.image(for: kind) {
                    Image(nsImage: image)
                        .renderingMode(.template)
                        .resizable()
                        .scaledToFit()
                        .foregroundStyle(.primary)
                } else {
                    monogram(color: kind.fallbackColor)
                }
            } else {
                Image(systemName: "server.rack")
                    .font(.system(size: size * 0.62, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private func monogram(color: Color) -> some View {
        ZStack {
            Circle()
                .fill(color.opacity(0.16))
            Text(monogramText)
                .font(.system(size: size * 0.52, weight: .semibold, design: .rounded))
                .foregroundStyle(color)
        }
    }

    private var monogramText: String {
        let trimmed = providerName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = trimmed.first else { return "?" }
        return String(first).uppercased()
    }
}
