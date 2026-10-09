// Three questions the Flick macOS design depends on. Never run on a Mac yet.
//   swift flick-probe.swift move    -- park every normal window off-screen, wait 2 s, put it back
//   swift flick-probe.swift tap     -- swallow mouse side buttons for 10 s, print movement deltas
//   swift flick-probe.swift shot    -- capture one window while it is parked off-screen
// Needs Accessibility (move, tap), Input Monitoring (tap), Screen Recording (shot)
// granted to the terminal that runs it.
import AppKit
import ApplicationServices

func windows() -> [(AXUIElement, String)] {
    var out: [(AXUIElement, String)] = []
    for app in NSWorkspace.shared.runningApplications where app.activationPolicy == .regular {
        let ax = AXUIElementCreateApplication(app.processIdentifier)
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &v) == .success,
              let list = v as? [AXUIElement] else { continue }
        for w in list { out.append((w, app.localizedName ?? "?")) }
    }
    return out
}

func position(_ w: AXUIElement) -> CGPoint? {
    var v: CFTypeRef?
    guard AXUIElementCopyAttributeValue(w, kAXPositionAttribute as CFString, &v) == .success else { return nil }
    var p = CGPoint.zero
    AXValueGetValue(v as! AXValue, .cgPoint, &p)
    return p
}

@discardableResult
func move(_ w: AXUIElement, to p: CGPoint) -> Bool {
    var p = p
    guard let v = AXValueCreate(.cgPoint, &p) else { return false }
    return AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, v) == .success
}

func ms(_ t: CFAbsoluteTime) -> String { String(format: "%.1f ms", (CFAbsoluteTimeGetCurrent() - t) * 1000) }

guard AXIsProcessTrusted() || CommandLine.arguments.last == "shot" else {
    print("NO ACCESSIBILITY: System Settings > Privacy & Security > Accessibility > allow this terminal")
    exit(2)
}

switch CommandLine.arguments.last {
case "move":
    let screen = NSScreen.main!.frame
    let corner = CGPoint(x: screen.maxX - 1, y: screen.maxY - 1)
    let all = windows().compactMap { w, app in position(w).map { (w, app, $0) } }
    var t = CFAbsoluteTimeGetCurrent()
    for (w, app, _) in all {
        let ok = move(w, to: corner)
        let now = position(w) ?? .zero
        print("park  \(app): \(ok ? "ok" : "REFUSED") -> \(Int(now.x)),\(Int(now.y))")
    }
    print("parked \(all.count) windows in \(ms(t))")
    sleep(2)
    t = CFAbsoluteTimeGetCurrent()
    for (w, app, home) in all where !move(w, to: home) { print("restore REFUSED: \(app)") }
    print("restored in \(ms(t))")

case "tap":
    let mask = (1 << CGEventType.otherMouseDown.rawValue) | (1 << CGEventType.otherMouseUp.rawValue)
        | (1 << CGEventType.mouseMoved.rawValue) | (1 << CGEventType.otherMouseDragged.rawValue)
    guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
        eventsOfInterest: CGEventMask(mask), callback: { _, type, e, _ in
            let button = e.getIntegerValueField(.mouseEventButtonNumber)
            let dx = e.getIntegerValueField(.mouseEventDeltaX), dy = e.getIntegerValueField(.mouseEventDeltaY)
            switch type {
            case .otherMouseDown where button >= 3:
                CGAssociateMouseAndMouseCursorPosition(0)   // freeze the pointer while held
                print("side button \(button) down (swallowed, pointer frozen)"); return nil
            case .otherMouseUp where button >= 3:
                CGAssociateMouseAndMouseCursorPosition(1)
                print("side button \(button) up"); return nil
            case .otherMouseDragged: print("delta \(dx),\(dy)")
            default: break
            }
            return Unmanaged.passUnretained(e)
        }, userInfo: nil) else {
        print("NO TAP: needs Accessibility and Input Monitoring"); exit(2)
    }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
    print("hold a mouse side button and move, 10 s")
    CFRunLoopRunInMode(.defaultMode, 10, false)
    CGAssociateMouseAndMouseCursorPosition(1)

case "shot":
    // Window ids come from the window list; the capture itself goes through screencapture -l
    // so this file needs no ScreenCaptureKit async code.
    let info = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
    let normal = info.filter { ($0[kCGWindowLayer as String] as? Int) == 0 && ($0[kCGWindowOwnerName as String] as? String) != nil }
    for w in normal.prefix(12) {
        let b = w[kCGWindowBounds as String] as? [String: CGFloat] ?? [:]
        print("\(w[kCGWindowNumber as String] ?? 0)\t\(w[kCGWindowOwnerName as String] ?? "")\tonscreen=\(w[kCGWindowIsOnscreen as String] ?? false)\tx=\(Int(b["X"] ?? 0)) y=\(Int(b["Y"] ?? 0))")
    }
    print("then: swift flick-probe.swift move &  sleep 1; screencapture -x -l <id> ~/borrow/out/parked.png")

default:
    print("usage: swift flick-probe.swift move|tap|shot")
}
