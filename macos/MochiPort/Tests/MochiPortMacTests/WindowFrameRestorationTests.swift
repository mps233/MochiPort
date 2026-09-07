import XCTest

#if canImport(MochiPortMac)
@testable import MochiPortMac
#elseif canImport(MochiPort)
@testable import MochiPort
#endif

/// 覆盖窗口恢复位置守护的纯几何判断。
/// 场景来源：装有模拟器虚拟屏（如 MuMu，位于主屏下方 y≥1080）时，
/// 上次关闭保存的窗口位置可能恢复到看不见的屏幕区域。
final class WindowFrameRestorationTests: XCTestCase {
    private let mainScreen = CGRect(x: 0, y: 0, width: 1920, height: 1055)

    func testFrameOnInvisibleVirtualDisplayRelocatesToMainScreenCenter() {
        // 主屏 1080pt 高；窗口整体落在下方虚拟屏区域。
        let frame = CGRect(x: 784, y: 1106, width: 1135, height: 719)
        let origin = WindowFrameRestoration.relocatedOrigin(frame: frame, screens: [mainScreen])
        XCTAssertNotNil(origin)
        XCTAssertEqual(origin!.x, mainScreen.midX - frame.width / 2, accuracy: 0.5)
        XCTAssertEqual(origin!.y, mainScreen.midY - frame.height / 2, accuracy: 0.5)
    }

    func testMostlyVisibleFrameIsKept() {
        let frame = CGRect(x: 723, y: 225, width: 1135, height: 719)
        XCTAssertNil(WindowFrameRestoration.relocatedOrigin(frame: frame, screens: [mainScreen]))
    }

    func testBarelyOverlappingFrameRelocates() {
        // 只有底部 100pt（约 8%）落在主屏内，低于保留阈值。
        let frame = CGRect(x: 100, y: 955, width: 1135, height: 719)
        let origin = WindowFrameRestoration.relocatedOrigin(frame: frame, screens: [mainScreen])
        XCTAssertNotNil(origin)
        XCTAssertTrue(mainScreen.contains(CGRect(origin: origin!, size: frame.size)))
    }

    func testRelocatesToScreenWithLargestOverlap() {
        // 两块屏都存在，但窗口悬在右侧屏幕右缘之外：与侧屏仅有 30pt 重叠
        // （约 2.6%），应拉回重叠最多的侧屏并居中。
        let sideScreen = CGRect(x: 1920, y: 0, width: 1920, height: 1055)
        let frame = CGRect(x: 3810, y: 200, width: 1135, height: 719)
        let origin = WindowFrameRestoration.relocatedOrigin(frame: frame, screens: [mainScreen, sideScreen])
        XCTAssertNotNil(origin)
        XCTAssertTrue(sideScreen.contains(CGRect(origin: origin!, size: frame.size)))
    }

    func testOversizedWindowAlignsToScreenTopLeadingCorner() {
        let tinyScreen = CGRect(x: 0, y: 0, width: 800, height: 600)
        let frame = CGRect(x: 2500, y: 2500, width: 1200, height: 900)
        let origin = WindowFrameRestoration.relocatedOrigin(frame: frame, screens: [tinyScreen])
        XCTAssertEqual(origin, CGPoint(x: 0, y: 0))
    }

    func testDegenerateInputsAreKept() {
        XCTAssertNil(
            WindowFrameRestoration.relocatedOrigin(
                frame: CGRect(x: 500, y: 500, width: 1135, height: 719),
                screens: []
            )
        )
        XCTAssertNil(
            WindowFrameRestoration.relocatedOrigin(
                frame: CGRect(x: 500, y: 500, width: 0, height: 0),
                screens: [mainScreen]
            )
        )
    }
}
