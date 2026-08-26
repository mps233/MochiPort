import SwiftUI

#if DEBUG
#Preview("概览 - 可用") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .available))
        .frame(width: 1040, height: 700)
}

#Preview("概览 - 服务不可用") {
    RootView()
        .environmentObject(AppModel(fixtureStatus: .unavailable("预览：后台服务已离线")))
        .frame(width: 1040, height: 700)
}
#endif
