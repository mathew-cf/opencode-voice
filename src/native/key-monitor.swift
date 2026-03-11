// key-monitor — Lightweight global hotkey monitor for opencode-voice.
//
// Uses a CGEventTap (listen-only) to detect keydown/keyup for a single key,
// then prints JSON events to stdout.  The parent Bun process reads these
// lines to drive push-to-talk recording.
//
// Usage:  key-monitor <keycode>
//   keycode = macOS virtual keycode (decimal), e.g. 61 for Right Option.
//
// Output (one JSON object per line):
//   {"event":"ready"}              — tap is active, monitoring
//   {"event":"keydown"}            — target key pressed
//   {"event":"keyup"}              — target key released
//
// Requires Accessibility permission (System Settings > Privacy & Security).
// Exits with code 1 and a JSON error on stderr if not granted.
//
// Build:
//   swiftc -O -o key-monitor key-monitor.swift -framework Cocoa

import Cocoa

// ---------------------------------------------------------------------------
// Context struct passed to the C callback via userInfo/refcon.
// CGEventTapCallBack is a C function pointer so it cannot capture Swift
// variables — all mutable state lives here.
// ---------------------------------------------------------------------------

struct MonitorContext {
    var targetKeycode: Int64
    var isModifierKey: Bool
    var isPressed: Bool
    var eventTap: CFMachPort?
}

// ---------------------------------------------------------------------------
// Modifier key detection
// ---------------------------------------------------------------------------
// Modifier keys fire CGEventType.flagsChanged instead of keyDown/keyUp.

let modifierKeycodes: Set<Int64> = [
    0x37,  // Left Command
    0x36,  // Right Command
    0x38,  // Left Shift
    0x3C,  // Right Shift
    0x3A,  // Left Option
    0x3D,  // Right Option
    0x3B,  // Left Control
    0x3E,  // Right Control
    0x3F,  // Function (fn)
    0x39,  // Caps Lock
]

// ---------------------------------------------------------------------------
// Parse arguments
// ---------------------------------------------------------------------------

guard CommandLine.arguments.count >= 2,
      let targetKeycode = Int64(CommandLine.arguments[1]) else {
    fputs("{\"event\":\"error\",\"message\":\"Usage: key-monitor <keycode>\"}\n", stderr)
    exit(1)
}

// ---------------------------------------------------------------------------
// Accessibility check
// ---------------------------------------------------------------------------

let options: NSDictionary = [
    kAXTrustedCheckOptionPrompt.takeRetainedValue(): true
]

guard AXIsProcessTrustedWithOptions(options) else {
    fputs("{\"event\":\"error\",\"message\":\"Accessibility permission required. Grant access in System Settings > Privacy & Security > Accessibility.\"}\n", stderr)
    exit(1)
}

// ---------------------------------------------------------------------------
// Allocate context on the heap so the C callback can access it safely.
// ---------------------------------------------------------------------------

let ctxPtr = UnsafeMutablePointer<MonitorContext>.allocate(capacity: 1)
ctxPtr.initialize(to: MonitorContext(
    targetKeycode: targetKeycode,
    isModifierKey: modifierKeycodes.contains(targetKeycode),
    isPressed: false,
    eventTap: nil
))

// ---------------------------------------------------------------------------
// Event callback (C function pointer — no captures allowed)
// ---------------------------------------------------------------------------

func eventCallback(
    proxy: CGEventTapProxy,
    type: CGEventType,
    event: CGEvent,
    refcon: UnsafeMutableRawPointer?
) -> Unmanaged<CGEvent>? {
    guard let refcon = refcon else {
        return Unmanaged.passRetained(event)
    }

    let ctx = refcon.assumingMemoryBound(to: MonitorContext.self)

    // Re-enable tap if macOS disabled it (e.g. timeout).
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
        if let tap = ctx.pointee.eventTap {
            CGEvent.tapEnable(tap: tap, enable: true)
        }
        return Unmanaged.passRetained(event)
    }

    let kc = event.getIntegerValueField(.keyboardEventKeycode)
    guard kc == ctx.pointee.targetKeycode else {
        return Unmanaged.passRetained(event)
    }

    if ctx.pointee.isModifierKey {
        // Modifier keys: flagsChanged fires once on press, once on release.
        if !ctx.pointee.isPressed {
            ctx.pointee.isPressed = true
            print("{\"event\":\"keydown\"}")
            fflush(stdout)
        } else {
            ctx.pointee.isPressed = false
            print("{\"event\":\"keyup\"}")
            fflush(stdout)
        }
    } else {
        // Regular keys: distinct keyDown / keyUp events.
        if type == .keyDown && !ctx.pointee.isPressed {
            ctx.pointee.isPressed = true
            print("{\"event\":\"keydown\"}")
            fflush(stdout)
        } else if type == .keyUp && ctx.pointee.isPressed {
            ctx.pointee.isPressed = false
            print("{\"event\":\"keyup\"}")
            fflush(stdout)
        }
    }

    return Unmanaged.passRetained(event)
}

// ---------------------------------------------------------------------------
// Create event tap
// ---------------------------------------------------------------------------

var eventMask: CGEventMask
if ctxPtr.pointee.isModifierKey {
    eventMask = (1 << CGEventType.flagsChanged.rawValue)
} else {
    eventMask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue)
}

guard let tap = CGEvent.tapCreate(
    tap: .cgSessionEventTap,
    place: .headInsertEventTap,
    options: .listenOnly,
    eventsOfInterest: eventMask,
    callback: eventCallback,
    userInfo: ctxPtr
) else {
    fputs("{\"event\":\"error\",\"message\":\"Failed to create event tap. Check Accessibility permissions.\"}\n", stderr)
    ctxPtr.deallocate()
    exit(1)
}

ctxPtr.pointee.eventTap = tap

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

let runLoopSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
CFRunLoopAddSource(CFRunLoopGetCurrent(), runLoopSource, .commonModes)
CGEvent.tapEnable(tap: tap, enable: true)

// Signal readiness to parent process.
print("{\"event\":\"ready\"}")
fflush(stdout)

// Clean shutdown on SIGINT / SIGTERM.
signal(SIGINT)  { _ in exit(0) }
signal(SIGTERM) { _ in exit(0) }

CFRunLoopRun()
