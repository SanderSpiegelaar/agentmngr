// Supplement system_profiler with CoreGraphics' current mode and spike window ID.
import CoreGraphics
import Foundation

var count: UInt32 = 0
CGGetActiveDisplayList(0, nil, &count)
var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
CGGetActiveDisplayList(count, &displays, &count)
for id in displays {
    let mode = CGDisplayCopyDisplayMode(id)
    print("display=\(id) main=\(CGDisplayIsMain(id)) builtin=\(CGDisplayIsBuiltin(id)) bounds=\(CGDisplayBounds(id)) refresh_hz=\(mode?.refreshRate ?? 0)")
}
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
for w in windows {
    let owner = w[kCGWindowOwnerName as String] as? String ?? ""
    let title = w[kCGWindowName as String] as? String ?? ""
    if owner.contains("matrix_spike") || title.contains("T-003 Matrix") {
        print("window=\(w[kCGWindowNumber as String] ?? "?") owner=\(owner) title=\(title) bounds=\(w[kCGWindowBounds as String] ?? "?")")
    }
}
