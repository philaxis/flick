// Public macOS feature probes, not the Flick application.
// Run from the borrowed account's Terminal so GUI permissions belong to Terminal.
// Apple preflight APIs report this process's access, not another app's permissions.
import AppKit
import ApplicationServices
import AVFoundation
import Darwin

let mode = CommandLine.arguments.last ?? "permissions"
let out = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("borrow/out")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

func permissions() {
    print("손쉬운 사용\t\(AXIsProcessTrusted() ? "허용" : "미허용")\t친구: 설정 > 개인정보 보호 및 보안 > 손쉬운 사용 > 이 실행 주체 허용")
    print("입력 모니터링\t\(CGPreflightListenEventAccess() ? "허용" : "미허용")\t친구: 설정 > 개인정보 보호 및 보안 > 입력 모니터링 > Terminal 허용")
    print("화면 기록\t\(CGPreflightScreenCaptureAccess() ? "허용" : "미허용")\t친구: 설정 > 개인정보 보호 및 보안 > 화면 기록 > Terminal 허용")
    print("마이크\t\(AVCaptureDevice.authorizationStatus(for: .audio).rawValue) (이 실행 주체)\t친구: 물빛 앱에서 마이크 허용 여부 별도 확인")
    print("권한 조회는 이 프로세스 기준. SSH에서 본 상태는 물빛 앱·Terminal의 허용을 보장하지 않습니다.")
}

func windows() -> [(AXUIElement, pid_t)] {
    var out: [(AXUIElement, pid_t)] = []
    for app in NSWorkspace.shared.runningApplications where app.activationPolicy == .regular {
        let ax = AXUIElementCreateApplication(app.processIdentifier)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &value) == .success,
              let list = value as? [AXUIElement] else { continue }
        for w in list { out.append((w, app.processIdentifier)) }
    }
    return out
}

func position(_ w: AXUIElement) -> CGPoint? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(w, kAXPositionAttribute as CFString, &value) == .success,
          let value = value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var p = CGPoint.zero
    guard AXValueGetValue(value as! AXValue, .cgPoint, &p) else { return nil }
    return p
}

@discardableResult
func move(_ w: AXUIElement, to p: CGPoint) -> Bool {
    var p = p
    guard let v = AXValueCreate(.cgPoint, &p) else { return false }
    return AXUIElementSetAttributeValue(w, kAXPositionAttribute as CFString, v) == .success
}

func corner() -> CGPoint? {
    guard !NSScreen.screens.isEmpty else { return nil }
    // Place on the outer corner of the full display union, including multiple monitors.
    let rect = NSScreen.screens.reduce(CGRect.null) { $0.union($1.frame) }
    return CGPoint(x: rect.maxX - 1, y: rect.maxY - 1)
}

func moveTest() -> Int32 {
    guard AXIsProcessTrusted(), let target = corner() else { print("확인 못 함: 손쉬운 사용 또는 화면 세션 없음"); return 2 }
    let all = windows().compactMap { w, pid in position(w).map { (w, pid, $0) } }
    guard !all.isEmpty else { print("확인 못 함: 움직일 일반 창 없음"); return 2 }
    var restored = false
    defer { if !restored { for (w, _, p) in all { move(w, to: p) } } }
    let start = CFAbsoluteTimeGetCurrent()
    var refused = 0
    for (w, _, _) in all { if !move(w, to: target) { refused += 1 } }
    print("창 이동 \(all.count)개 · 거부 \(refused)개 · \((CFAbsoluteTimeGetCurrent() - start) * 1000) ms")
    sleep(2)
    var restoreFailed = 0
    for (w, _, p) in all { if !move(w, to: p) { restoreFailed += 1 } }
    restored = restoreFailed == 0
    print("제자리 복원 · 실패 \(restoreFailed)개. 깜빡임·체감 속도는 친구가 화면에서 확인해야 합니다.")
    return refused == 0 && restored ? 0 : 2
}

var sideDown = 0, sideUp = 0, deltas = 0
var eventTap: CFMachPort?
func tapTest() -> Int32 {
    guard AXIsProcessTrusted() else { print("확인 못 함: 손쉬운 사용 미허용"); return 2 }
    let mask = (1 << CGEventType.otherMouseDown.rawValue) | (1 << CGEventType.otherMouseUp.rawValue)
        | (1 << CGEventType.mouseMoved.rawValue) | (1 << CGEventType.otherMouseDragged.rawValue)
    eventTap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
        eventsOfInterest: CGEventMask(mask), callback: { _, type, event, _ in
            if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                CGAssociateMouseAndMouseCursorPosition(1)
                if let tap = eventTap { CGEvent.tapEnable(tap: tap, enable: true) }
                return Unmanaged.passUnretained(event)
            }
            let button = event.getIntegerValueField(.mouseEventButtonNumber)
            switch type {
            case .otherMouseDown where button >= 3:
                sideDown += 1; CGAssociateMouseAndMouseCursorPosition(0)
                print("옆 단추 누름 · 이벤트 삼킴 · 포인터 고정"); return nil
            case .otherMouseUp where button >= 3:
                sideUp += 1; CGAssociateMouseAndMouseCursorPosition(1)
                print("옆 단추 놓음 · 포인터 복원"); return nil
            case .otherMouseDragged, .mouseMoved:
                if sideDown > sideUp { deltas += 1 }
            default: break
            }
            return Unmanaged.passUnretained(event)
        }, userInfo: nil)
    guard let tap = eventTap else { print("확인 못 함: 입력 훅 생성 거부. Terminal 권한을 확인하세요."); return 2 }
    defer { CGAssociateMouseAndMouseCursorPosition(1); CFMachPortInvalidate(tap) }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
    CGEvent.tapEnable(tap: tap, enable: true)
    print("친구: 10초 동안 마우스 옆 단추를 누른 채 움직였다 놓으세요.")
    CFRunLoopRunInMode(.defaultMode, 10, false)
    print("입력 훅 생성됨 · 누름 \(sideDown) · 놓음 \(sideUp) · 이동량 이벤트 \(deltas)")
    if sideDown == 0 || sideUp == 0 || deltas == 0 { print("확인 못 함: 실제 마우스 조작 데이터 부족"); return 2 }
    return 0
}

func shotTest() -> Int32 {
    guard AXIsProcessTrusted(), CGPreflightScreenCaptureAccess(), let target = corner() else {
        print("확인 못 함: 손쉬운 사용·화면 기록·화면 세션이 필요합니다."); return 2
    }
    let infos = (CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
    for (w, pid) in windows() {
        guard let home = position(w), let info = infos.first(where: {
            guard ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid),
                  ($0[kCGWindowLayer as String] as? Int) == 0,
                  let b = $0[kCGWindowBounds as String] as? [String: CGFloat] else { return false }
            return abs((b["X"] ?? -99999) - home.x) < 3 && abs((b["Y"] ?? -99999) - home.y) < 3
        }), let id = info[kCGWindowNumber as String] as? Int else { continue }
        defer { if !move(w, to: home) { print("복원 실패: 친구가 창을 화면으로 옮겨 주세요.") } }
        guard move(w, to: target) else { print("확인 못 함: 창 이동 거부"); return 2 }
        let file = out.appendingPathComponent("parked.png").path
        try? FileManager.default.removeItem(atPath: file)
        let task = Process(); task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        task.arguments = ["-x", "-l", String(id), file]
        do { try task.run(); task.waitUntilExit() } catch { print("창 캡처 실행 실패"); return 2 }
        let size = (try? FileManager.default.attributesOfItem(atPath: file)[.size] as? Int) ?? 0
        guard task.terminationStatus == 0, size > 0 else { print("확인 못 함: 화면 밖 창 캡처 실패"); return 2 }
        print("화면 밖 창 단위 캡처 파일 생성됨: ~/borrow/out/parked.png (\(size) bytes). 실제 내용은 ./mac pull로 회수해 확인하세요.")
        return 0
    }
    print("확인 못 함: AX 창과 캡처 창 번호가 대응되는 일반 창 없음"); return 2
}

switch mode {
case "permissions": permissions()
case "request":
    if !AXIsProcessTrusted() { _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary) }
    if !CGPreflightListenEventAccess() { _ = CGRequestListenEventAccess() }
    if !CGPreflightScreenCaptureAccess() { _ = CGRequestScreenCaptureAccess() }
    print("친구: 권한 창·설정에서 Terminal을 허용한 뒤 ./mac flick-check로 다시 확인하세요.")
case "move": exit(moveTest())
case "tap": exit(tapTest())
case "shot": exit(shotTest())
default: print("usage: flick-probe permissions|request|move|tap|shot"); exit(2)
}
