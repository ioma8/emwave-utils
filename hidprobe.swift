import Foundation
import IOKit.hid
import CoreFoundation

// Probe HeartMath emWave2 (VID 0x0E30, PID 0x0008):
//  - print device properties + HID report descriptor
//  - stream input reports for N seconds (default 5)

let VID: Int = 0x0E30
let PID: Int = 0x0008
let seconds: Int = CommandLine.arguments.count > 1 ? Int(CommandLine.arguments[1]) ?? 5 : 5

let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
let openErr = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
print("open: \(openErr)")
IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
let spinEnd = Date().addingTimeInterval(1.0)
while Date() < spinEnd {
    CFRunLoopRunInMode(CFRunLoopMode.defaultMode, 0.1, false)
}
guard let all = IOHIDManagerCopyDevices(manager) as? [IOHIDDevice] else {
    print("no devices at all")
    exit(1)
}
print("total HID devices: \(all.count)")
let devices = all.filter { dev in
    guard let v = IOHIDDeviceGetProperty(dev, kIOHIDVendorIDKey as CFString) as? Int,
          let p = IOHIDDeviceGetProperty(dev, kIOHIDProductIDKey as CFString) as? Int else { return false }
    return v == VID && p == PID
}
guard !devices.isEmpty else {
    print("emWave2 not in HID list; listing all:")
    for dev in all {
        let v = IOHIDDeviceGetProperty(dev, kIOHIDVendorIDKey as CFString).map { String(describing: $0) } ?? "?"
        let p = IOHIDDeviceGetProperty(dev, kIOHIDProductIDKey as CFString).map { String(describing: $0) } ?? "?"
        let prod = IOHIDDeviceGetProperty(dev, kIOHIDProductKey as CFString).map { String(describing: $0) } ?? "?"
        print("  vid=\(v) pid=\(p) product=\(prod)")
    }
    exit(1)
}

for dev in devices {
    func prop(_ key: String) -> String {
        guard let v = IOHIDDeviceGetProperty(dev, key as CFString) else { return "?" }
        if let d = v as? Data { return d.map { String(format: "%02x", $0) }.joined() }
        return String(describing: v)
    }
    print("=== device ===")
    print("product:       \(prop(kIOHIDProductKey))")
    print("manufacturer:  \(prop(kIOHIDManufacturerKey))")
    print("usage page:    \(prop(kIOHIDPrimaryUsagePageKey))")
    print("usage:         \(prop(kIOHIDPrimaryUsageKey))")
    print("max input:     \(prop(kIOHIDMaxInputReportSizeKey))")
    print("max output:    \(prop(kIOHIDMaxOutputReportSizeKey))")
    print("max feature:   \(prop(kIOHIDMaxFeatureReportSizeKey))")
    let rd = prop(kIOHIDReportDescriptorKey)
    print("report desc:   \(rd)")
    print("locationID:    \(prop(kIOHIDLocationIDKey))")
    print("serial:        \(prop(kIOHIDSerialNumberKey))")
    print("transport:     \(prop(kIOHIDTransportKey))")

    var buf = [UInt8](repeating: 0, count: 16384)
    var count = 0
    let ctx = Unmanaged.passUnretained(NSNumber(value: 0)).toOpaque()
    let callback: IOHIDReportCallback = { context, result, sender, type, reportID, reportPtr, reportLen in
        let data = Data(bytes: reportPtr, count: reportLen)
        print("report id=\(reportID) type=\(type.rawValue) len=\(reportLen): \(data.map { String(format: "%02x", $0) }.joined())")
    }
    IOHIDDeviceRegisterInputReportCallback(dev, &buf, buf.count, callback, ctx)
    let devOpen = IOHIDDeviceOpen(dev, IOOptionBits(kIOHIDOptionsTypeNone))
    print("device open: \(devOpen)")

    // dump feature reports 0x00..0x0F to probe for bootloader/debug hooks
    for rid in 0...15 {
        var fb = [UInt8](repeating: 0, count: 4096)
        var fbLen = fb.count
        let r = IOHIDDeviceGetReport(dev, kIOHIDReportTypeFeature, rid, &fb, &fbLen)
        if r == kIOReturnSuccess {
            let data = Data(fb.prefix(fb.count))
            print("feature[\(rid)]: \(data.map { String(format: "%02x", $0) }.joined())")
        }
    }

    let runLoop = RunLoop.current
    let end = Date().addingTimeInterval(TimeInterval(seconds))
    while Date() < end {
        runLoop.run(mode: .default, before: Date().addingTimeInterval(0.2))
        count += 1
    }
    print("stream done after \(seconds)s")
    IOHIDDeviceClose(dev, IOOptionBits(kIOHIDOptionsTypeNone))
}
IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
