//
// TokenCompanionAnimator.swift
//
// Dashboard hero-card companion: a spring-driven blob character ported from
// the study engine at blessonism/grok-icon-study (repository author approved
// the port). Character geometry lives in CompanionBlobGeometry and remains
// the property of xAI; it must not be presented as original MochiPort artwork.
//
// The engine keeps the replica's motion model: damped springs for body
// placement and squash, a blink keyframe queue, per-mood eye-shape playlists
// with random hold times, gaze drift, occasional winks, and tricks (spin,
// hop, dizzy). Rendering is one Canvas path per frame: the body outline with
// the two eye polygons punched out via the even-odd fill rule.
//

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

// MARK: - Springs

/// Damped harmonic oscillator, integrated at 120 Hz substeps like the source
/// engine (`stepSpring` with DT = 1/120).
struct CompanionSpring {
    var x: Double
    var v: Double
    var t: Double

    init(_ x: Double) {
        self.x = x
        self.v = 0
        self.t = x
    }

    mutating func step(freq: Double, damp: Double, dt: Double) {
        v += (-2 * damp * freq * v - freq * freq * (x - t)) * dt
        x += v * dt
        if !x.isFinite || !v.isFinite {
            x = t
            v = 0
        }
    }
}

// MARK: - Moods

/// Motion moods. The companion states map onto the replica moods whose pose
/// and eye playlists fit the dashboard role.
enum CompanionMood: Hashable {
    case idle
    case working
    case listening
    case celebrate
    case alerting
    case sleeping
    case happy

    static func from(_ state: TokenCompanionState) -> CompanionMood {
        switch state {
        case .idle: return .idle
        case .working: return .working
        case .waiting: return .listening
        case .success: return .celebrate
        case .error: return .alerting
        case .disconnected: return .sleeping
        case .happy: return .happy
        }
    }

    /// Eye-shape indices cycled per mood (EYE_PLAYLIST).
    var eyePlaylist: [Int] {
        switch self {
        case .idle: return [0, 8]
        case .working: return [7, 16, 11, 10]
        case .listening: return [10, 1, 19]
        case .celebrate: return [2, 8, 17]
        case .alerting: return [3, 21]
        case .sleeping: return [13, 22, 4]
        case .happy: return [2, 11, 17, 19]
        }
    }

    /// Random hold per eye shape, in ms (EYE_HOLD_MS).
    var eyeHoldMs: ClosedRange<Double> {
        switch self {
        case .idle: return 9000...16000
        case .working: return 1800...3200
        case .listening: return 2800...5000
        case .celebrate: return 1400...2600
        case .alerting: return 2000...3600
        case .sleeping: return 6000...10000
        case .happy: return 2500...4500
        }
    }

    /// Blink cadence, in ms; nil = no spontaneous blinks (BLINK_MS).
    var blinkCadenceMs: ClosedRange<Double>? {
        switch self {
        case .idle: return 6000...14000
        case .working: return 2800...5500
        case .listening: return 3000...7000
        case .celebrate: return 2200...4500
        case .alerting, .sleeping: return nil
        case .happy: return 2500...5000
        }
    }

    /// Moods that schedule idle tricks (V_T / B_T).
    var schedulesTricks: Bool { self == .happy }
}

// MARK: - Idle styles

/// 待机（idle）动画风格，设置页可选。每种风格定义自己的眨眼节奏、wink
/// 频率、随机小动作和视线习惯；姿态差异见引擎 applyPose 的 .idle 分支。
public enum CompanionIdleStyle: String, CaseIterable, Hashable {
    case classic
    case lively
    case sleepy
    case curious
    case groovy

    public var label: String {
        switch self {
        case .classic: return "经典"
        case .lively: return "活泼"
        case .sleepy: return "慵懒"
        case .curious: return "好奇"
        case .groovy: return "律动"
        }
    }

    /// 自发 wink 的间隔；nil 表示待机时不眨单眼。
    var winkEveryMs: ClosedRange<Double>? {
        switch self {
        case .classic: return 4500...10000
        case .lively: return 2200...4800
        case .sleepy: return nil
        case .curious: return 3500...7000
        case .groovy: return 5000...9000
        }
    }

    /// 随机小动作（hop/转圈/弹跳转）的间隔；nil 表示待机时来小动作，
    /// 好奇风格的小动作是姿态里的歪头，不在这里排期。
    var trickEveryMs: ClosedRange<Double>? {
        switch self {
        case .classic: return nil
        case .lively: return 7000...14000
        case .sleepy: return nil
        case .curious: return nil
        case .groovy: return nil
        }
    }

    /// 自发眨眼的节奏；全部风格都眨，只是快慢不同。
    var blinkCadenceMs: ClosedRange<Double> {
        switch self {
        case .classic: return 6000...14000
        case .lively: return 4000...9000
        case .sleepy: return 11000...18000
        case .curious: return 4500...9000
        case .groovy: return 5000...10000
        }
    }

    /// 每种风格自己的表情轮播组合，取自原版对应情绪的眼形表：
    /// classic=idle、lively=playful、sleepy=drowsy、curious=curious、groovy=laughing。
    var eyePlaylist: [Int] {
        switch self {
        case .classic: return [0, 8]
        case .lively: return [2, 17, 11, 8]
        case .sleepy: return [4, 22, 13]
        case .curious: return [3, 21, 0, 15]
        case .groovy: return [2, 11, 17]
        }
    }

    /// 表情保持时长。比原版 idle（9–16 秒）短，让眼形变化看得见；
    /// 好奇最快，慵懒最慢。
    var eyeHoldMs: ClosedRange<Double> {
        switch self {
        case .classic: return 6000...11000
        case .lively: return 2200...3800
        case .sleepy: return 5000...8000
        case .curious: return 1800...3200
        case .groovy: return 2400...4000
        }
    }
}

// MARK: - Tricks

struct CompanionTrick {
    enum Kind {
        case spinBounce
        case spinDizzy
        case spinWild
    }

    var kind: Kind
    var t0: Double
    var dir: Double
    var turns: Int
}

/// Trick output per tick (evalTrick): body-rotation wobble `kr` (deg), offsets
/// `yi`/`ki` (geometry units), eye drift `eyeDX`/`eyeDY`, lid multiplier, and
/// the face-orbit angle `turn` (radians, shared with spin turns).
struct CompanionTrickFrame {
    var turn: Double?
    var kr: Double = 0
    var yi: Double = 0
    var ki: Double = 0
    var eyeDX: Double = 0
    var eyeDY: Double = 0
    var lidMul: Double?
    var eyeBoost: Double?
    var wantHop = false
    var done = false
}

// MARK: - Engine

/// One rendered frame: group placement plus fully transformed eye polygons in
/// geometry space. `eyeVisible[side] == false` means the face has rotated the
/// eye behind the silhouette.
struct CompanionFrame {
    var tx: Double = 0
    var ty: Double = 0
    var rot: Double = 0
    var squashY: Double = 1
    var eyePoints: [[CGPoint]] = [[], []]
    var eyeVisible: [Bool] = [true, true]
}

final class CompanionEngine {
    private(set) var frame = CompanionFrame()

    private var mood: CompanionMood = .idle

    private var spin = CompanionSpring(0)
    private var tx = CompanionSpring(0)
    private var ty = CompanionSpring(0)
    private var squash = CompanionSpring(1)
    private var blink = CompanionSpring(1)
    private var eyeScale = CompanionSpring(1)
    private var gazeX = CompanionSpring(0)
    private var gazeY = CompanionSpring(0)
    private var eyeMorph = CompanionSpring(1)
    private var spinTurn: CompanionSpring?

    private var eyeFrom = 0
    private var eyeTo = 0
    private var eyeIdx = 0
    private var eyeStiffness = 7.0
    private var fromPolys: [[CGPoint]]?

    // Clocks in ms.
    private var t0: Double = 0
    private var stateAt: Double = 0
    private var last: Double = 0
    private var eyeUntil: Double = 0
    private var blinkUntil = Double.infinity
    private var gazeUntil: Double = 0
    private var winkUntil: Double = 0
    private var winkAt = -1e9
    private var winkEye = 0
    private var trickAt: Double = 0
    private var trick: CompanionTrick?
    private var hopAt: Double = -1
    private var celebrateAt: Double = -1
    private var blinkQueue: [(at: Double, v: Double)] = []

    // Per-mood context (applyPose's ctx).
    private var impulseAt: Double = 0
    private var angryShakeUntil: Double = 0
    private var pendingTyKick: Double = 0
    private var forceSleepEye = false
    private var nodUntil: Double = 0
    private var nodEnd: Double = 0
    private var stAt: Double = 0
    private var wantPn: (turns: Int, dir: Double)?
    // 好奇风格的周期性歪头。
    private var leanUntil: Double = 0
    private var leanEnd: Double = 0
    private var leanDir: Double = 1

    /// 待机动画风格，影响 idle 情绪下的姿态与节奏。可随时切换。
    var idleStyle: CompanionIdleStyle = .classic

    /// 当前生效的表情轮播：待机时跟随风格，其余状态用情绪自己的表。
    private var activePlaylist: [Int] {
        mood == .idle ? idleStyle.eyePlaylist : mood.eyePlaylist
    }

    /// 当前生效的表情保持时长。
    private var activeEyeHoldMs: ClosedRange<Double> {
        mood == .idle ? idleStyle.eyeHoldMs : mood.eyeHoldMs
    }

    init(now: Double) {
        t0 = now
        setState(.idle, now: now, resetEyes: true)
        last = now
    }

    /// Restart the mood timers (setState). `resetEyes` snaps to the playlist
    /// head instead of morphing.
    func setState(_ mood: CompanionMood, now: Double, resetEyes: Bool = false) {
        self.mood = mood
        stateAt = now
        let list = activePlaylist
        eyeIdx = 0
        if resetEyes {
            eyeFrom = list[0]
            eyeTo = list[0]
            fromPolys = nil
            eyeMorph.x = 1
            eyeMorph.t = 1
            eyeMorph.v = 0
        } else if mood != .sleeping {
            morphEyes(list[0], stiffness: 8)
        }
        eyeUntil = now + Double.random(in: activeEyeHoldMs)
        if let cadence = mood.blinkCadenceMs {
            blinkUntil = now + Double.random(in: 1500...7000)
        } else {
            blinkUntil = .infinity
        }
        gazeUntil = now + Double.random(in: 500...1400)
        winkUntil = now + Double.random(in: 3000...8000)
        impulseAt = now + Double.random(in: 500...1200)
        angryShakeUntil = 0
        pendingTyKick = 0
        forceSleepEye = false
        nodUntil = now + 1800
        nodEnd = 0
        stAt = now + (mood == .working
            ? Double.random(in: 1200...2400)
            : Double.random(in: 6000...10000))
        wantPn = nil
        celebrateAt = mood == .celebrate ? now + 140 : -1
        trick = nil
        spinTurn = nil
        hopAt = -1
        if mood != .sleeping {
            queueBlink(at: now)
        }
    }

    /// Advance the simulation to `now` (ms) and recompute `frame`.
    func tick(now: Double) {
        if last == 0 { last = now }
        let dt = min((now - last) / 1000, 0.1)
        last = now

        let mt = (now - t0) / 1000
        let dtState = (now - stateAt) / 1000
        let pose = applyPose(mt: mt, dtState: dtState, now: now)
        spin.t = pose.spin
        tx.t = pose.tx
        ty.t = pose.ty
        squash.t = pose.squash
        eyeScale.t = pose.eyeBoost
        if pendingTyKick != 0 {
            ty.v += pendingTyKick
            pendingTyKick = 0
        }
        if forceSleepEye {
            forceSleepEye = false
            morphEyes(13, stiffness: 11)
        }
        if let pn = wantPn {
            startSpin(turns: pn.turns, dir: pn.dir)
            wantPn = nil
        }

        if celebrateAt > 0 && now >= celebrateAt && trick == nil && spinTurn == nil {
            trick = startTrick(.spinWild, turns: 9, now: now)
            celebrateAt = now + 6200
        }

        let trickRange: ClosedRange<Double>? = mood.schedulesTricks
            ? 9000...18000
            : (mood == .idle ? idleStyle.trickEveryMs : nil)
        if now >= trickAt {
            if trickRange != nil && spinTurn == nil && hopAt < 0 && trick == nil {
                if mood.schedulesTricks {
                    if Double.random(in: 0..<1) < 0.55 {
                        startSpin(turns: 1, dir: Double.random(in: 0..<1) < 0.5 ? -1 : 1)
                    } else {
                        trick = startTrick(.spinBounce, turns: 1, now: now)
                    }
                } else {
                    // 活泼待机：跳一下 / 原地转一圈 / 弹跳转。
                    let z = Double.random(in: 0..<1)
                    if z < 0.5 {
                        hopAt = now
                    } else if z < 0.8 {
                        startSpin(turns: 1, dir: Double.random(in: 0..<1) < 0.5 ? -1 : 1)
                    } else {
                        trick = startTrick(.spinBounce, turns: 1, now: now)
                    }
                }
            }
            trickAt = now + (trickRange.map { Double.random(in: $0) } ?? 15000)
        }

        var tf = evalTrick(now: now)
        if tf.wantHop && hopAt < 0 { hopAt = now }
        if tf.done { trick = nil }
        var hop = hopY(at: now)
        if hop == nil {
            hopAt = -1
            hop = 0
        }
        var turn = tf.turn
        if let st = spinTurn {
            turn = (turn ?? 0) + st.x
            if abs(st.t - st.x) < 0.004 && abs(st.v) < 0.015 {
                spinTurn = nil
            }
        }
        if let boost = tf.eyeBoost { eyeScale.t = boost }

        if mood != .sleeping && now >= eyeUntil {
            let list = activePlaylist
            let stride = max(1, list.count - 1)
            eyeIdx = (eyeIdx + 1 + Int(Double.random(in: 0..<Double(stride)))) % list.count
            let stiffness: Double = (mood == .idle && idleStyle == .curious) ? 10 : 6
            morphEyes(list[eyeIdx], stiffness: stiffness)
            eyeUntil = now + Double.random(in: activeEyeHoldMs)
        }

        let cadence = mood == .idle ? Optional(idleStyle.blinkCadenceMs) : mood.blinkCadenceMs
        if let cadence, now >= blinkUntil {
            queueBlink(at: now)
            blinkUntil = now + Double.random(in: cadence)
        }
        let blinkKey = consumeBlink(now: now)
        blink.t = blinkKey ?? (blinkQueue.isEmpty ? (tf.lidMul ?? pose.lid) : blink.t)

        if now >= gazeUntil {
            let gz = nextGaze()
            gazeX.t = gz.x
            gazeY.t = gz.y
            gazeUntil = now + Double.random(in: gz.hold)
        }

        let winkRange: ClosedRange<Double>? = mood == .happy
            ? 4500...10000
            : (mood == .idle ? idleStyle.winkEveryMs : nil)
        if let range = winkRange, now >= winkUntil {
            winkAt = now
            winkEye = Double.random(in: 0..<1) < 0.5 ? 0 : 1
            winkUntil = now + Double.random(in: range)
        }

        // Integrate at 120 Hz substeps.
        let nSteps = max(1, Int(ceil(dt / (1.0 / 120.0))))
        let step = dt / Double(nSteps)
        for _ in 0..<nSteps {
            eyeMorph.step(freq: eyeStiffness, damp: 1, dt: step)
            if var st = spinTurn {
                st.step(freq: 6.2, damp: 1, dt: step)
                spinTurn = st
            }
            spin.step(freq: 5, damp: 0.9, dt: step)
            tx.step(freq: 3.5, damp: 1, dt: step)
            ty.step(freq: 4, damp: 1, dt: step)
            squash.step(freq: 10, damp: 0.8, dt: step)
            blink.step(freq: 26, damp: 1, dt: step)
            eyeScale.step(freq: 9, damp: 0.85, dt: step)
            gazeX.step(freq: 13, damp: 1, dt: step)
            gazeY.step(freq: 13, damp: 1, dt: step)
        }

        frame = paint(
            now: now,
            morphT: min(max(eyeMorph.x, 0), 1),
            blinkBase: blink.x,
            turn: turn,
            hop: hop ?? 0,
            txExtra: tf.yi,
            tyExtra: tf.ki,
            rotExtra: tf.kr,
            eyeDX: tf.eyeDX,
            eyeDY: tf.eyeDY
        )
    }

    /// 切换待机风格后立即轮换到新表情表的第一组，避免沿用旧风格的眼睛。
    func adoptIdleStyle(_ style: CompanionIdleStyle, now: Double) {
        idleStyle = style
        guard mood == .idle else { return }
        eyeIdx = 0
        morphEyes(activePlaylist[0], stiffness: 8)
        eyeUntil = now + Double.random(in: activeEyeHoldMs)
    }

    /// One static frame: mood applied with no motion, eyes at the playlist
    /// head, matching the source engine's reduce-motion rendering.
    func staticFrame() {
        frame = paint(
            now: 0,
            morphT: 1,
            blinkBase: 1,
            turn: nil,
            hop: 0,
            txExtra: 0,
            tyExtra: 0,
            rotExtra: 0,
            eyeDX: 0,
            eyeDY: 0,
            polysOverride: currentPolys(1)
        )
    }

    // MARK: Pose (applyPose)

    private struct PoseResult {
        var spin: Double = 0
        var tx: Double = 0
        var ty: Double = 0
        var squash: Double = 1
        var lid: Double = 1
        var eyeBoost: Double = 1
    }

    private func applyPose(mt: Double, dtState: Double, now: Double) -> PoseResult {
        var r = PoseResult()
        switch mood {
        case .idle:
            switch idleStyle {
            case .classic:
                r.spin = sin(mt * 0.5) * 1.5 + sin(mt * 0.17) * 0.6
                r.tx = sin(mt * 0.27)
                r.ty = sin(mt * 0.85) * 1.2
                r.squash = 1 + sin(mt * 0.85) * 0.007
            case .lively:
                r.spin = sin(mt * 1.1) * 3.5 + sin(mt * 0.23) * 0.8
                r.tx = sin(mt * 0.9) * 2.2
                r.ty = -abs(sin(mt * 1.7)) * 2.2
                r.squash = 1 + sin(mt * 1.7) * 0.012
            case .sleepy:
                r.spin = sin(mt * 0.22) * 1.2
                r.tx = sin(mt * 0.16) * 0.8
                r.ty = 1.5 + sin(mt * 0.3) * 1.8
                r.squash = 1 + sin(mt * 0.3) * 0.02
                r.lid = 0.55 + sin(mt * 0.4) * 0.06
            case .curious:
                if now >= leanUntil {
                    leanUntil = now + Double.random(in: 2800...5600)
                    leanEnd = now + 620
                    leanDir = Double.random(in: 0..<1) < 0.5 ? -1 : 1
                }
                var leanSpin = 0.0
                var leanTx = 0.0
                if now < leanEnd {
                    let e = 1 - (leanEnd - now) / 620
                    leanSpin = sin(e * .pi) * 6 * leanDir
                    leanTx = sin(e * .pi) * 3 * leanDir
                }
                r.spin = sin(mt * 0.5) * 2 + leanSpin
                r.tx = sin(mt * 0.6) * 2.5 + leanTx
                r.ty = -1.5 + sin(mt * 0.9) * 1.2
                r.squash = 1.01
                r.eyeBoost = 1.06
            case .groovy:
                let beat = sin(mt * .pi * 2)
                r.ty = -abs(beat) * 2.8
                r.squash = 1 - abs(beat) * 0.02
                r.spin = sin(mt * .pi) * 4
                r.tx = sin(mt * .pi) * 2.4
            }
        case .working:
            let e = sin(mt * .pi * 2 * 1.6)
            r.spin = 4 + e * 2.5
            r.tx = 3
            r.ty = 1.5 + max(0, e) * 3
            r.squash = 1 - max(0, e) * 0.02
            if now >= stAt {
                wantPn = (1, 1)
                stAt = now + Double.random(in: 6000...9000)
            }
        case .listening:
            r.spin = 8 + sin(mt * 0.5) * 1.5
            r.tx = 2
            r.ty = -2 + sin(mt * 0.8) * 0.8
            r.squash = 1.015
            if now >= nodUntil {
                nodUntil = now + Double.random(in: 1800...3200)
                nodEnd = now + 380
            }
            if now < nodEnd {
                let e = 1 - (nodEnd - now) / 380
                r.ty += sin(e * .pi) * 4.5
                r.spin += sin(e * .pi) * 2
            }
        case .celebrate:
            r.ty = -abs(sin(mt * 1.6)) * 2.5
            r.squash = 1
            r.eyeBoost = 1.1
            r.lid = 1.1
        case .alerting:
            if now >= impulseAt {
                angryShakeUntil = now + 420
                pendingTyKick = 70
                impulseAt = now + Double.random(in: 1800...3200)
            }
            r.spin = now < angryShakeUntil ? sin(now * 0.05) * 4.5 : 0
            r.tx = 0
            r.ty = 3.5
            r.squash = 0.975
        case .sleeping:
            let en = min(dtState / 2, 1)
            let settle = sin(min(max(dtState / 0.5, 0), 1) * .pi)
            r.spin = 4 * en + sin(mt * 0.25) * 2
            r.tx = -2 * en
            r.ty = 8 * en + sin(mt * 0.55) * 3 - settle * 5
            r.squash = 1 + sin(mt * 0.55) * 0.016 + settle * 0.05
            if mood.eyePlaylist.contains(eyeTo) {
                r.lid = eyeMorph.x > 0.85 ? 1 : 0.08
            } else if dtState < 1.2 {
                let dn = min(1, dtState)
                r.lid = max(0.08, 1 - dn * (1 + 0.15 * sin(dtState * 6.5)))
            } else {
                r.lid = 0.08
                if blink.x < 0.18 { forceSleepEye = true }
            }
        case .happy:
            let e = sin(mt * 2.4)
            r.spin = sin(mt * 1.2) * 3
            r.tx = sin(mt * 1.1) * 2.5
            r.ty = -abs(e) * 3
            r.squash = 1 + e * 0.02
            r.eyeBoost = 1.05
        }
        return r
    }

    /// Random gaze targets per mood (nextGaze). Idle defers to the style.
    private func nextGaze() -> (x: Double, y: Double, hold: ClosedRange<Double>) {
        if mood == .idle {
            let sign: Double = Double.random(in: 0..<1) < 0.5 ? -1 : 1
            switch idleStyle {
            case .classic:
                return (0, 0, 2500...5500)
            case .lively:
                return (Double.random(in: -0.4...0.4) * 15, Double.random(in: -0.3...0.3) * 9, 2000...4000)
            case .sleepy:
                return (0, 2.5, 4000...7000)
            case .curious:
                return (sign * Double.random(in: 0.6...1) * 15, Double.random(in: -1...1) * 9, 950...1900)
            case .groovy:
                return (0, 0, 3000...5000)
            }
        }
        switch mood {
        case .idle:
            return (0, 0, 2500...5500)
        case .listening:
            return (Double.random(in: -0.3...0.3) * 15, Double.random(in: -0.25...0.25) * 9, 2200...4200)
        case .working:
            return (Double.random(in: -0.4...0.4) * 15, Double.random(in: 0.4...1) * 9, 1200...2400)
        case .happy:
            return (Double.random(in: -0.7...0.7) * 15, -Double.random(in: 0...0.6) * 9, 1800...3400)
        default:
            return (Double.random(in: -0.4...0.4) * 15, Double.random(in: -0.3...0.3) * 9, 2500...5000)
        }
    }

    // MARK: Tricks

    private func startSpin(turns: Int, dir: Double) {
        guard spinTurn == nil else { return }
        var s = CompanionSpring(0)
        s.t = Double(turns) * 2 * .pi * dir
        spinTurn = s
    }

    private func startTrick(_ kind: CompanionTrick.Kind, turns: Int, now: Double) -> CompanionTrick {
        let dir: Double = Double.random(in: 0..<1) < 0.5 ? -1 : 1
        let resolved: Int
        switch kind {
        case .spinDizzy: resolved = Int(Double.random(in: 3...4).rounded())
        case .spinWild: resolved = 9
        case .spinBounce: resolved = turns
        }
        return CompanionTrick(kind: kind, t0: now, dir: dir, turns: resolved)
    }

    private func evalTrick(now: Double) -> CompanionTrickFrame {
        guard let t = trick else { return CompanionTrickFrame() }
        let e = (now - t.t0) / 1000
        var f = CompanionTrickFrame()
        switch t.kind {
        case .spinBounce:
            if e < 0.7 {
                f.turn = Double(t.turns) * 2 * .pi * t.dir * easeInOutCubic(e / 0.7)
            } else {
                f.wantHop = true
                f.done = true
            }
        case .spinDizzy:
            let on = 0.55 + Double(t.turns) * 0.16
            let bn = 1.5
            if e < on {
                let c = e / on
                f.turn = Double(t.turns) * 2 * .pi * t.dir * (c * c)
            } else if e < on + bn {
                let c = e - on
                let bi = pow(1 - c / bn, 1.3)
                f.kr = sin(c * 10) * 17 * t.dir * bi
                f.yi = cos(c * 10) * 10 * t.dir * bi
                f.ki = sin(c * 20) * 3 * bi
                f.lidMul = 0.46 + 0.14 * sin(c * 21)
                f.eyeBoost = 1.03
            } else {
                f.done = true
            }
        case .spinWild:
            // Source: wild nine-turn spin, wind-up glide, wobble settle.
            let pc = 0.5
            let go = 2.0 * Double.pi
            let end = 0.24 + 2.3 + 1.25
            let gm = (Double(t.turns) * go + pc) / (0.3 / 2 + 2.0 + 1.25 / 4)
            if e < end + 1.7 {
                var cr: Double
                if e < 0.24 {
                    cr = -pc * (1 - cos((e / 0.24) * .pi)) / 2
                } else if e < 0.54 {
                    let ua = e - 0.24
                    cr = -pc + gm * ua * ua / (2 * 0.3)
                } else if e < 2.54 {
                    cr = -pc + gm * (0.3 / 2 + (e - 0.24 - 0.3))
                } else if e < end {
                    let ua = (e - 0.24 - 2.3) / 1.25
                    cr = -pc + gm * (0.3 / 2 + 2.0) + gm * 1.25 * (1 - pow(1 - ua, 4)) / 4
                } else {
                    cr = Double(t.turns) * go
                }
                f.turn = cr * t.dir
                var pl = 0.0
                if e > 2.54 {
                    let ua = min((e - 2.54) / 1.25, 1)
                    pl = ua < 0.4 ? 0 : pow((ua - 0.4) / 0.6, 2)
                    if e >= end { pl = pow(1 - (e - end) / 1.7, 1.6) }
                }
                let yl = max(e - 2.54, 0)
                f.kr = sin(yl * 9.2) * 11 * t.dir * pl
                f.yi = (cos(yl * 9.2) - 1) * 6 * t.dir * pl
                f.ki = sin(yl * 18.4) * 2.6 * pl
                f.eyeDX = sin(yl * 11.5) * 13 * t.dir * pl
                f.eyeDY = (cos(yl * 9) - 1) * 3.5 * pl
                f.lidMul = 1.14 - 0.44 * pl + 0.1 * sin(yl * 16) * pl
                f.eyeBoost = 1.12 - 0.09 * pl
            } else {
                f.done = true
            }
        }
        return f
    }

    /// Decaying parabolic hops (hopY): 48/28/14/6-unit arcs over 1.33 s.
    private func hopY(at now: Double) -> Double? {
        guard hopAt >= 0 else { return nil }
        let e = (now - hopAt) / 1000
        let segs: [(h: Double, d: Double)] = [(48, 0.5), (28, 0.382), (14, 0.27), (6, 0.177)]
        var elapsed = 0.0
        for seg in segs {
            if e < elapsed + seg.d {
                let b = (e - elapsed) / seg.d
                return -4 * seg.h * b * (1 - b)
            }
            elapsed += seg.d
        }
        return nil
    }

    // MARK: Blink queue (queueBlink / consumeBlink)

    private func queueBlink(at now: Double) {
        var keys: [(at: Double, v: Double)] = [
            (at: now, v: 0.05),
            (at: now + 70, v: 0.05),
            (at: now + 150, v: 1.08),
            (at: now + 300, v: 1),
        ]
        if Double.random(in: 0..<1) < 0.14 {
            keys.append((at: now + 370, v: 0.05))
            keys.append((at: now + 480, v: 1))
        }
        blinkQueue.append(contentsOf: keys)
    }

    private func consumeBlink(now: Double) -> Double? {
        var key: Double?
        while let first = blinkQueue.first, now >= first.at {
            key = blinkQueue.removeFirst().v
        }
        return key
    }

    // MARK: Eye morph

    private func currentPolys(_ t: Double) -> [[CGPoint]] {
        let eyes = CompanionBlobGeometry.eyes
        guard eyes.count > max(eyeFrom, eyeTo) else { return [[], []] }
        let from = fromPolys ?? eyes[eyeFrom]
        let to = eyes[eyeTo]
        let tt = CGFloat(t)
        func lerp(_ a: CGPoint, _ b: CGPoint) -> CGPoint {
            CGPoint(x: a.x + (b.x - a.x) * tt, y: a.y + (b.y - a.y) * tt)
        }
        return [
            zip(from[0], to[0]).map(lerp),
            zip(from[1], to[1]).map(lerp),
        ]
    }

    private func morphEyes(_ index: Int, stiffness: Double) {
        if index == eyeTo && eyeMorph.t == 1 && eyeMorph.x >= 1 { return }
        let t = min(max(eyeMorph.x, 0), 1)
        eyeFrom = eyeTo
        fromPolys = currentPolys(t)
        eyeTo = index
        eyeMorph.x = 0
        eyeMorph.v = 0
        eyeMorph.t = 1
        eyeStiffness = stiffness
    }

    // MARK: Paint (paintEyes + group transform, flat 2D path)

    /// Face tuning for the blob with the login-wrap look (FACE_TUNE).
    private static let faceGap = 1.18
    private static let faceEyeWidth = 0.96
    private static let faceEyeHeight = 0.92

    private func paint(
        now: Double,
        morphT: Double,
        blinkBase: Double,
        turn: Double?,
        hop: Double,
        txExtra: Double,
        tyExtra: Double,
        rotExtra: Double,
        eyeDX: Double,
        eyeDY: Double,
        polysOverride: [[CGPoint]]? = nil
    ) -> CompanionFrame {
        let geo = CompanionBlobGeometry.self
        let re = geo.center
        let top = 0.01
        let bottom = 228.44

        var frame = CompanionFrame()
        frame.tx = tx.x + txExtra
        frame.ty = ty.x + hop + tyExtra
        frame.rot = spin.x + rotExtra
        frame.squashY = squash.x

        let polys = polysOverride ?? currentPolys(min(max(eyeMorph.x, 0), 1))
        guard polys.count == 2, !polys[0].isEmpty, !polys[1].isEmpty else {
            frame.eyeVisible = [false, false]
            return frame
        }

        let pulse = 1 + 0.07 * sin(morphT * .pi)
        let cents = polys.map { centroid($0) }
        var halfWidths: [Double] = []
        for (i, poly) in polys.enumerated() {
            let a = poly.map { abs(Double($0.x) - cents[i].x) }.max() ?? 0
            halfWidths.append(a)
        }
        let l1 = abs(cents[1].x - cents[0].x) * Self.faceGap
        let total = halfWidths[0] + halfWidths[1]
        let maxScale = total > 0.5 ? min(max(l1 / total, 0.35), 4) : 4
        let oX = min(min(max(eyeScale.x, 0.2), 2), maxScale / pulse)
        let hee = oX * Self.faceEyeWidth
        let u1 = oX * Self.faceEyeHeight

        for i in 0...1 {
            let poly = polys[i]
            let (gn, ti) = (Double(cents[i].x), Double(cents[i].y))

            // Lid: blink spring, squashed by a wink on one eye.
            var lid = max(blinkBase, 0.04)
            if i == winkEye && now < winkAt + 320 {
                let xr = (now - winkAt) / 320
                let envelope = xr < 0.42 ? 1 - xr / 0.42 : (xr - 0.42) / 0.58
                lid = max(lid * min(max(envelope, 0), 1), 0.04)
            }

            // Placement with the optional face-orbit projection (turn).
            var ca = re
            var wo = (gn - re) * Self.faceGap
            let sre = min(max(re + (ti - re), top + 2), bottom - 2)
            var tre: Double = 1
            var eyeSquish: Double = 1
            var visible = true
            if let turn {
                let (spL, spR) = geo.span(y: sre)
                let rad = max((spR - spL) / 2, 12)
                ca = (spL + spR) / 2
                let li0 = asin(min(max(wo / rad, -1), 1))
                let bl0 = li0 + turn
                let io0 = cos(bl0)
                let uo0 = max(cos(li0), 0.02)
                visible = io0 > 0.02
                eyeSquish = max(io0, 0.02) / uo0
                wo = rad * sin(bl0)
                tre = smoothStep(min(max(io0 / 0.5, 0), 1))
            }

            // Micro-wobble plus gaze drift.
            let kj = sin(now * 42e-5 + Double(i)) * 1.4 + sin(now * 0.001 + Double(i) * 2) * 0.5
                + gazeX.x + eyeDX
            let ko = sin(now * 58e-5 + Double(i)) * 0.9 + gazeY.x + eyeDY

            let vee = min(max(eyeSquish * hee * pulse, 0.02), 2.4)
            let v2 = min(max(lid * u1 * pulse, 0.02), 2.4)
            let ume = geo.eyeMargin * v2 + 2
            let vl: Double
            if turn != nil {
                vl = min(max(sre + ko, top + ume), bottom - ume)
            } else {
                vl = min(max(re + (ti + ko - re), top + ume), bottom - ume)
            }

            // Clamp the eye centre so every point stays on the body.
            var o2 = -Double.infinity
            var xl = Double.infinity
            for p in poly {
                let scanY = vl + (Double(p.y) - ti) * v2
                let edges = geo.span(y: scanY)
                let dx = (Double(p.x) - gn) * vee
                o2 = max(o2, edges.left - dx)
                xl = min(xl, edges.right - dx)
            }
            let xre = ca + wo + kj * Self.faceGap
            let lx = o2 <= xl ? min(max(xre, o2), xl) : (o2 + xl) / 2
            let dd = lx + (xre - lx) * (1 - tre)

            frame.eyePoints[i] = poly.map { CGPoint(
                x: (Double($0.x) - gn) * vee + dd,
                y: (Double($0.y) - ti) * v2 + vl
            )}
            frame.eyeVisible[i] = visible
        }
        return frame
    }

    private func centroid(_ pts: [CGPoint]) -> CGPoint {
        guard !pts.isEmpty else { return .zero }
        var x = 0.0, y = 0.0
        for p in pts {
            x += Double(p.x)
            y += Double(p.y)
        }
        return CGPoint(x: x / Double(pts.count), y: y / Double(pts.count))
    }

    private func easeInOutCubic(_ n: Double) -> Double {
        n < 0.5 ? 4 * n * n * n : 1 - pow(-2 * n + 2, 3) / 2
    }

    private func smoothStep(_ n: Double) -> Double {
        n * n * (3 - 2 * n)
    }
}

// MARK: - View

public struct TokenCompanionAnimator: View {
    @Binding private var externalState: TokenCompanionState
    @State private var internalState: TokenCompanionState = .idle
    @State private var engine: CompanionEngine
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme

    /// Whether the spring simulation runs. Motion costs a full hosting-view
    /// layout per frame (measured ~14% CPU with the menu-bar window open,
    /// versus ~0% static), so it stays opt-in via the settings toggle.
    private let animates: Bool
    private let usesExternalBinding: Bool
    /// 待机动画风格，透传给引擎；切换时实时生效。
    private let idleStyle: CompanionIdleStyle

    /// 30fps. `.periodic` is Timer-backed, so this is the actual update rate
    /// rather than a hint the display link may ignore.
    private static let frameInterval: TimeInterval = 1.0 / 30.0

    private static func nowMs() -> Double {
        Date().timeIntervalSinceReferenceDate * 1000
    }

    public init() {
        self.init(state: .constant(.idle), animates: false)
    }

    public init(animates: Bool, idleStyle: CompanionIdleStyle = .classic) {
        self.init(state: .constant(.idle), animates: animates, idleStyle: idleStyle)
    }

    public init(
        state: Binding<TokenCompanionState>,
        animates: Bool = false,
        idleStyle: CompanionIdleStyle = .classic
    ) {
        self._externalState = state
        self.usesExternalBinding = true
        self.animates = animates
        self.idleStyle = idleStyle
        let engine = CompanionEngine(now: Self.nowMs())
        engine.idleStyle = idleStyle
        self._engine = State(initialValue: engine)
    }

    private var currentState: TokenCompanionState {
        usesExternalBinding ? externalState : internalState
    }

    public var body: some View {
        Group {
            if animates && !reduceMotion {
                TimelineView(.periodic(from: .now, by: Self.frameInterval)) { context in
                    Canvas { canvas, size in
                        engine.tick(now: context.date.timeIntervalSinceReferenceDate * 1000)
                        draw(engine.frame, in: &canvas, size: size)
                    }
                }
            } else {
                Canvas { canvas, size in
                    let staticEngine = CompanionEngine(now: 0)
                    staticEngine.idleStyle = idleStyle
                    staticEngine.setState(CompanionMood.from(currentState), now: 0, resetEyes: true)
                    staticEngine.staticFrame()
                    draw(staticEngine.frame, in: &canvas, size: size)
                }
            }
        }
        .frame(width: 76, height: 58)
        .onAppear {
            // `.constant` bindings never fire onChange, so the initial state
            // has to be pushed into the engine here.
            engine.setState(CompanionMood.from(currentState), now: Self.nowMs(), resetEyes: true)
        }
        .onChange(of: currentState) { _, newValue in
            engine.setState(CompanionMood.from(newValue), now: Self.nowMs())
        }
        .onChange(of: idleStyle) { _, newStyle in
            engine.adoptIdleStyle(newStyle, now: Self.nowMs())
        }
        .accessibilityHidden(true)
    }

    /// Ink gradient per mood and color scheme, following the source engine's
    /// login ink table (135° CSS direction: lighter at top-trailing).
    private func bodyGradient(for mood: CompanionMood) -> Gradient {
        let dark = colorScheme == .dark
        let top: Color
        let bottom: Color
        switch mood {
        case .alerting:
            // Red ink: light #FF5667→#E02135, dark #FF3E51→#A21826.
            top = dark ? Color(red: 1.0, green: 0.243, blue: 0.318)
                       : Color(red: 1.0, green: 0.337, blue: 0.404)
            bottom = dark ? Color(red: 0.635, green: 0.094, blue: 0.149)
                          : Color(red: 0.878, green: 0.129, blue: 0.208)
        case .sleeping:
            // Gray ink: light #A6A6A6→#696969, dark #B7B7B7→#777777.
            top = dark ? Color(red: 0.718, green: 0.718, blue: 0.718)
                       : Color(red: 0.651, green: 0.651, blue: 0.651)
            bottom = dark ? Color(red: 0.467, green: 0.467, blue: 0.467)
                          : Color(red: 0.412, green: 0.412, blue: 0.412)
        default:
            // Black ink: light #585858→#000, dark #FFFFFF→#C2C2C2.
            top = dark ? .white : Color(red: 0.345, green: 0.345, blue: 0.345)
            bottom = dark ? Color(red: 0.761, green: 0.761, blue: 0.761) : .black
        }
        return Gradient(colors: [top, bottom])
    }

    private func draw(_ frame: CompanionFrame, in canvas: inout GraphicsContext, size: CGSize) {
        let geo = CompanionBlobGeometry.self
        let re = geo.center

        // Fit the 229-unit blob with room for hops above and sleep sag below.
        let scale = Double(min((size.width - 6) / 228.6, (size.height - 11) / 232.4))
        let cx = Double(size.width) / 2
        let cy = Double(size.height) - 2.2 - 232.4 * scale / 2

        var combined = CGMutablePath()
        combined.addPath(geo.blobCGPath)
        for (i, pts) in frame.eyePoints.enumerated() where frame.eyeVisible[i] && !pts.isEmpty {
            let eye = CGMutablePath()
            eye.addLines(between: pts)
            eye.closeSubpath()
            combined.addPath(eye)
        }

        canvas.drawLayer { layer in
            // Geometry → canvas placement, then the body motion transform
            // (translate R+tx, R+ty; rotate; scale(1, squash); translate -R).
            layer.concatenate(CGAffineTransform(translationX: cx, y: cy)
                .scaledBy(x: scale, y: scale)
                .translatedBy(x: -re, y: -re))
            layer.concatenate(CGAffineTransform(translationX: re + frame.tx, y: re + frame.ty)
                .rotated(by: CGFloat(frame.rot * .pi / 180))
                .scaledBy(x: 1, y: CGFloat(frame.squashY))
                .translatedBy(x: -re, y: -re))

            let mood = CompanionMood.from(currentState)
            layer.fill(
                Path(combined),
                with: .linearGradient(
                    bodyGradient(for: mood),
                    startPoint: CGPoint(x: 228.5, y: 0),
                    endPoint: CGPoint(x: 0, y: 232.4)
                ),
                style: FillStyle(eoFill: true)
            )
        }
    }
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
