//
// TokenCompanionAnimator.swift
//
// A small, self-contained SwiftUI companion for the dashboard hero card.
// The character is intentionally built from one squishy mochi silhouette so it
// stays soft and clear at the dashboard's 76 x 58 point size.

import SwiftUI

public enum TokenCompanionState: String, CaseIterable, Hashable {
    case idle
    case working
    case waiting
    case success
    case error
    case disconnected

    // Kept for callers that used the previous generated state machine.
    case happy
}

private struct ThreadBlobBody: Shape {
    var wobble: CGFloat = 0
    var squish: CGFloat = 0

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(wobble, squish) }
        set {
            wobble = newValue.first
            squish = newValue.second
        }
    }

    func path(in rect: CGRect) -> Path {
        let centerX = rect.midX + wobble * 1.4
        // Directly mapped from the Dango Daikazoku CodePen's 254 x 154 body
        // with its 7-point border. CSS resolves its `250 250 100 100` corner
        // radii to 118 / 49, which gives the characteristic large dome and
        // broad flat base rather than an ordinary rounded rectangle.
        let sourceWidth: CGFloat = 268
        let sourceHeight: CGFloat = 168
        let sourceTopRadius: CGFloat = 118
        let sourceBottomRadius: CGFloat = 49
        let width = min(rect.width * 0.82, 62.5) * (1 + squish * 0.2)
        let sourceScale = width / sourceWidth
        let height = sourceHeight * sourceScale * (1 - squish * 0.1)
        let top = rect.midY - height * 0.5
        let bottom = rect.midY + height * 0.5
        let left = centerX - width * 0.5
        let right = centerX + width * 0.5
        let topRadiusX = sourceTopRadius * sourceScale
        let topRadiusY = sourceTopRadius * sourceScale * (1 - squish * 0.1)
        let bottomRadiusX = sourceBottomRadius * sourceScale
        let bottomRadiusY = sourceBottomRadius * sourceScale * (1 - squish * 0.1)
        let kappa: CGFloat = 0.5522848

        var path = Path()
        path.move(to: CGPoint(x: left + topRadiusX, y: top))
        path.addLine(to: CGPoint(x: right - topRadiusX, y: top))
        path.addCurve(
            to: CGPoint(x: right, y: top + topRadiusY),
            control1: CGPoint(x: right - topRadiusX + topRadiusX * kappa, y: top),
            control2: CGPoint(x: right, y: top + topRadiusY - topRadiusY * kappa)
        )
        path.addLine(to: CGPoint(x: right, y: bottom - bottomRadiusY))
        path.addCurve(
            to: CGPoint(x: right - bottomRadiusX, y: bottom),
            control1: CGPoint(x: right, y: bottom - bottomRadiusY + bottomRadiusY * kappa),
            control2: CGPoint(x: right - bottomRadiusX + bottomRadiusX * kappa, y: bottom)
        )
        path.addLine(to: CGPoint(x: left + bottomRadiusX, y: bottom))
        path.addCurve(
            to: CGPoint(x: left, y: bottom - bottomRadiusY),
            control1: CGPoint(x: left + bottomRadiusX - bottomRadiusX * kappa, y: bottom),
            control2: CGPoint(x: left, y: bottom - bottomRadiusY + bottomRadiusY * kappa)
        )
        path.addLine(to: CGPoint(x: left, y: top + topRadiusY))
        path.addCurve(
            to: CGPoint(x: left + topRadiusX, y: top),
            control1: CGPoint(x: left, y: top + topRadiusY - topRadiusY * kappa),
            control2: CGPoint(x: left + topRadiusX - topRadiusX * kappa, y: top)
        )
        path.closeSubpath()
        return path
    }
}

private struct ThreadBlobMotion {
    var offsetX: CGFloat = 0
    var offsetY: CGFloat = 0
    var scaleX: CGFloat = 1
    var scaleY: CGFloat = 1
    var rotation: Double = 0
    var wobble: CGFloat = 0
    var squish: CGFloat = 0
    var eyeOffsetX: CGFloat = 0
    var eyeScaleY: CGFloat = 1
}

private func clamp(_ value: Double, _ lower: Double = 0, _ upper: Double = 1) -> Double {
    min(max(value, lower), upper)
}

private func smoothStep(_ value: Double) -> Double {
    let t = clamp(value)
    return t * t * (3 - 2 * t)
}

private func pulse(_ phase: Double) -> Double {
    (sin(phase * 2 * .pi) + 1) / 2
}

private func dangoWiggle(_ elapsed: Double, duration: Double = 4) -> Double {
    let phase = (elapsed.truncatingRemainder(dividingBy: duration)) / duration

    if phase < 0.2 {
        return 5 * smoothStep(phase / 0.2)
    }
    if phase < 0.6 {
        return 5 - 9 * smoothStep((phase - 0.2) / 0.4)
    }
    return -4 + 4 * smoothStep((phase - 0.6) / 0.4)
}

private func motion(
    for state: TokenCompanionState,
    elapsed: Double,
    reactionElapsed: Double,
    reduceMotion: Bool
) -> ThreadBlobMotion {
    let idlePhase = elapsed / 3.8
    let breathe = reduceMotion ? 0 : sin(idlePhase * 2 * .pi) * 0.018
    let blinkPhase = elapsed.truncatingRemainder(dividingBy: 4) / 4
    let blink: Double
    if reduceMotion {
        blink = 0
    } else if blinkPhase < 0.1 {
        blink = smoothStep(blinkPhase / 0.1)
    } else if blinkPhase < 0.2 {
        blink = 1 - smoothStep((blinkPhase - 0.1) / 0.1)
    } else {
        blink = 0
    }
    let reaction = clamp(reactionElapsed / 0.86)
    let settle = 1 - smoothStep(reaction)

    var result = ThreadBlobMotion()
    result.scaleX = 1 + CGFloat(breathe)
    result.scaleY = 1 - CGFloat(breathe)
    result.eyeScaleY = CGFloat(blink)

    switch state {
    case .idle:
        if !reduceMotion {
            result.rotation = dangoWiggle(elapsed)
        }
    case .happy:
        break
    case .working:
        let workPhase = elapsed / 1.45
        result.offsetY -= CGFloat(pulse(workPhase) * 0.9)
        result.rotation = sin(workPhase * 2 * .pi) * 2.0
        result.eyeOffsetX = CGFloat(sin(workPhase * 2 * .pi) * 1.4)
    case .waiting:
        result.offsetY -= CGFloat(pulse(elapsed / 2.1) * 0.65)
        result.eyeOffsetX = CGFloat(sin(elapsed * 1.5) * 0.7)
    case .success:
        let bounce = sin(reaction * .pi) * settle
        result.offsetY -= CGFloat(bounce * 3.4)
        result.squish = CGFloat(sin(reaction * .pi) * 0.32 * settle)
        result.scaleX += CGFloat(sin(reaction * .pi) * 0.08 * settle)
        result.scaleY -= CGFloat(sin(reaction * .pi) * 0.08 * settle)
    case .error:
        let shake = sin(reaction * 5 * .pi) * settle
        result.offsetX = CGFloat(shake * 2.0)
        result.rotation = shake * 4
        result.scaleX += CGFloat(abs(shake) * 0.025)
    case .disconnected:
        result.offsetY += 1.4
        result.scaleY -= 0.025
        result.eyeScaleY = min(result.eyeScaleY, 0.32)
    }

    return result
}

public struct TokenCompanionAnimator: View {
    @Binding private var externalState: TokenCompanionState
    @State private var internalState: TokenCompanionState = .idle
    @State private var animationStart = Date()
    @State private var reactionStart = Date()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme

    /// Whether the breathing / blinking / reaction motion runs.
    ///
    /// The motion costs a full hosting-view layout per frame (measured ~14% CPU
    /// with the menu-bar window open, versus ~0% static), so it is opt-in via
    /// the settings toggle rather than always on.
    private let animates: Bool
    private let usesExternalBinding: Bool

    /// 30fps. `.periodic` is Timer-backed, so this is the actual update rate
    /// rather than a hint the display link may ignore.
    private static let frameInterval: TimeInterval = 1.0 / 30.0

    public init() {
        self._externalState = .constant(.idle)
        self.usesExternalBinding = false
        self.animates = false
    }

    public init(animates: Bool) {
        self._externalState = .constant(.idle)
        self.usesExternalBinding = false
        self.animates = animates
    }

    public init(state: Binding<TokenCompanionState>, animates: Bool = false) {
        self._externalState = state
        self.usesExternalBinding = true
        self.animates = animates
    }

    private var currentState: TokenCompanionState {
        usesExternalBinding ? externalState : internalState
    }

    public var body: some View {
        Group {
            if animates && !reduceMotion {
                TimelineView(.periodic(from: animationStart, by: Self.frameInterval)) { context in
                    let elapsed = max(0, context.date.timeIntervalSince(animationStart))
                    let reactionElapsed = max(0, context.date.timeIntervalSince(reactionStart))
                    Canvas { canvas, size in
                        drawCompanion(
                            context: &canvas,
                            size: size,
                            move: motion(
                                for: currentState,
                                elapsed: elapsed,
                                reactionElapsed: reactionElapsed,
                                reduceMotion: false
                            )
                        )
                    }
                }
            } else {
                // Static frame: no timeline, so the view is not re-laid-out.
                Canvas { context, size in
                    drawCompanion(
                        context: &context,
                        size: size,
                        move: motion(
                            for: currentState,
                            elapsed: 0,
                            reactionElapsed: 0,
                            reduceMotion: true
                        )
                    )
                }
            }
        }
        .frame(width: 76, height: 58)
        .onChange(of: currentState) { _, _ in reactionStart = Date() }
        .accessibilityHidden(true)
    }

    /// Draw the mascot for one state.
    ///
    /// `Canvas` resolves the shape and the face from the same `size`, so the
    /// body and eyes stay aligned exactly as the previous view hierarchy did.
    private func drawCompanion(
        context: inout GraphicsContext,
        size: CGSize,
        move: ThreadBlobMotion
    ) {
        let ink = colorScheme == .light
            ? Theme.mascotLightInk
            : Color(red: 0.306, green: 0.306, blue: 0.306)
        let frame = CGRect(origin: .zero, size: size)

        context.drawLayer { layer in
            // The previous modifiers used `.bottom` as the scale/rotation
            // anchor, so the body pivots on its base rather than its centre.
            let anchor = CGPoint(x: size.width / 2, y: size.height)
            layer.translateBy(x: anchor.x + move.offsetX, y: anchor.y + move.offsetY)
            layer.scaleBy(x: move.scaleX, y: move.scaleY)
            layer.rotate(by: .degrees(move.rotation))
            layer.translateBy(x: -anchor.x, y: -anchor.y)

            // Body.
            let body = ThreadBlobBody(wobble: move.wobble, squish: move.squish)
                .path(in: frame)
            layer.fill(body, with: .linearGradient(
                Gradient(colors: fillColors(for: currentState, colorScheme: colorScheme)),
                startPoint: CGPoint(x: frame.minX, y: frame.minY),
                endPoint: CGPoint(x: frame.maxX, y: frame.maxY)
            ))

            // Face: two capsules whose size morphs as the eyes close.
            // The previous HStack used `spacing = eyeCenterDistance * 2 - eyeWidth`,
            // so each eye centre sits at ±eyeCenterDistance from the middle.
            let bodyWidth: CGFloat = 62.5
            let bodyHeight: CGFloat = 39.18
            let closed = min(max(move.eyeScaleY, 0), 1)
            let openEyeWidth = bodyWidth * (8.5 / 268)
            let openEyeHeight = bodyHeight * (46 / 168)
            let closedEyeWidth = bodyWidth * (20.5 / 268)
            let closedEyeHeight = bodyHeight * (5.6 / 168)
            let eyeWidth = openEyeWidth + (closedEyeWidth - openEyeWidth) * closed
            let eyeHeight = openEyeHeight + (closedEyeHeight - openEyeHeight) * closed
            let eyeY = size.height / 2 - bodyHeight * (32 / 168)
                + bodyHeight * (7.5 / 168) * closed
            let eyeCenterDistance = bodyWidth * (20.5 / 268)
            let eyeOffset = move.eyeOffsetX
            for sign in [CGFloat(-1), CGFloat(1)] {
                let centerX = size.width / 2 + eyeOffset + sign * eyeCenterDistance
                let rect = CGRect(
                    x: centerX - eyeWidth / 2,
                    y: eyeY - eyeHeight / 2,
                    width: eyeWidth,
                    height: eyeHeight
                )
                layer.fill(Path(roundedRect: rect, cornerRadius: min(eyeWidth, eyeHeight) / 2), with: .color(ink))
            }
        }
    }
}

private func fillColors(for state: TokenCompanionState, colorScheme: ColorScheme) -> [Color] {
    let top: Color
    let bottom: Color

    switch state {
    case .error:
        top = Color(red: 1.0, green: 0.98, blue: 0.98)
        bottom = Color(red: 0.98, green: 0.84, blue: 0.84)
    case .disconnected:
        top = Color(red: 0.98, green: 0.98, blue: 0.98)
        bottom = Color(red: 0.87, green: 0.87, blue: 0.88)
    default:
        if colorScheme == .light {
            top = Theme.mascotLightFill
            bottom = Theme.mascotLightFill
        } else {
            top = Color.primary
            bottom = Color.primary
        }
    }

    return [top, bottom]
}

#if DEBUG
struct TokenCompanionAnimator_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 16) {
            TokenCompanionAnimator()
                .frame(width: 152, height: 116)
                .background(.white)
                .clipShape(RoundedRectangle(cornerRadius: 14))

            TokenCompanionAnimator(state: .constant(.working))
                .frame(width: 152, height: 116)
                .background(.black.opacity(0.8))
                .clipShape(RoundedRectangle(cornerRadius: 14))
        }
        .padding(24)
    }
}
#endif
