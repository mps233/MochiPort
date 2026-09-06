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
    @State private var animationStart = Date.distantPast
    @State private var reactionStart = Date.distantPast
    @State private var displayedState: TokenCompanionState = .idle
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme

    private let usesExternalBinding: Bool

    public init() {
        self._externalState = .constant(.idle)
        self.usesExternalBinding = false
    }

    public init(state: Binding<TokenCompanionState>) {
        self._externalState = state
        self.usesExternalBinding = true
    }

    private var currentState: TokenCompanionState {
        usesExternalBinding ? externalState : internalState
    }

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { context in
            let now = context.date
            let elapsed = max(0, now.timeIntervalSince(animationStart))
            let reactionElapsed = max(0, now.timeIntervalSince(reactionStart))
            let state = displayedState
            let move = motion(
                for: state,
                elapsed: elapsed,
                reactionElapsed: reactionElapsed,
                reduceMotion: reduceMotion
            )

            ZStack {
                Group {
                    ThreadBlobBody(wobble: move.wobble, squish: move.squish)
                        .fill(bodyFill(for: state, colorScheme: colorScheme))
                        .frame(width: 76, height: 58)

                    ThreadBlobFace(
                        eyeOffsetX: move.eyeOffsetX,
                        eyeScaleY: move.eyeScaleY,
                        colorScheme: colorScheme
                    )
                }
                .scaleEffect(x: move.scaleX, y: move.scaleY, anchor: .bottom)
                .rotationEffect(.degrees(move.rotation), anchor: .bottom)
                .offset(x: move.offsetX, y: move.offsetY)
            }
            .frame(width: 76, height: 58)
        }
        .onAppear {
            let now = Date()
            animationStart = now
            reactionStart = now
            displayedState = currentState
        }
        .onChange(of: currentState) { _, newState in
            displayedState = newState
            reactionStart = Date()
        }
        .accessibilityHidden(true)
    }

    // Kept for source compatibility with the old generated view.
    public mutating func setState(_ state: TokenCompanionState) {
        internalState = state
    }
}

private struct ThreadBlobFace: View {
    let eyeOffsetX: CGFloat
    let eyeScaleY: CGFloat
    let colorScheme: ColorScheme

    private var ink: Color {
        colorScheme == .light
            ? Theme.mascotLightInk
            : Color(red: 0.306, green: 0.306, blue: 0.306)
    }

    // Exact relative measurements of the CodePen's outer 268 x 168 dango.
    private let bodyWidth: CGFloat = 62.5
    private let bodyHeight: CGFloat = 39.18

    private var closedAmount: CGFloat {
        min(max(eyeScaleY, 0), 1)
    }

    var body: some View {
        ZStack {
            let openEyeWidth = bodyWidth * (8.5 / 268)
            let openEyeHeight = bodyHeight * (46 / 168)
            let closedEyeWidth = bodyWidth * (20.5 / 268)
            let closedEyeHeight = bodyHeight * (5.6 / 168)
            let eyeWidth = openEyeWidth + (closedEyeWidth - openEyeWidth) * closedAmount
            let eyeHeight = openEyeHeight + (closedEyeHeight - openEyeHeight) * closedAmount
            let eyeY = -bodyHeight * (32 / 168) + bodyHeight * (7.5 / 168) * closedAmount
            let eyeCenterDistance = bodyWidth * (20.5 / 268)

            HStack(spacing: eyeCenterDistance * 2 - eyeWidth) {
                Capsule()
                    .fill(ink)
                    .frame(width: eyeWidth, height: eyeHeight)
                Capsule()
                    .fill(ink)
                    .frame(width: eyeWidth, height: eyeHeight)
            }
            .offset(x: eyeOffsetX, y: eyeY)
        }
        .frame(width: 76, height: 58)
    }
}

private func bodyFill(for state: TokenCompanionState, colorScheme: ColorScheme) -> LinearGradient {
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

    return LinearGradient(
        colors: [top, bottom],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )
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
