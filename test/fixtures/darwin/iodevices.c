// darwin/devices-page's program: IOKit's registry and HID manager over the
// page's `devices` service (page/runtime/src/devices.js), as a Mac program
// uses them, built against the SDK's IOKit. The page gives it a HID game
// pad and a USB device; it lists what the registry has of them, then
// through an IOHIDManager (matching two products) and a matching and a
// termination notification it hears the pad matched, reads its elements,
// gets its feature report, and sets an output report, at which the page
// sends an input report and gives it a second pad; it hears that one
// arrive, sets another output report, at which the page takes the second
// pad away; it hears it leave, and ends. Each step's lines are printed in
// an order of their own, whatever order the run loop's sources fired in.
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOKitLib.h>
#include <IOKit/hid/IOHIDKeys.h>
#include <IOKit/hid/IOHIDManager.h>
#include <IOKit/usb/IOUSBLib.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static long num(CFTypeRef v) {
    long long n = -1;
    if (v && CFGetTypeID(v) == CFNumberGetTypeID()) CFNumberGetValue(v, kCFNumberLongLongType, &n);
    return (long)n;
}

static const char *str(CFTypeRef v, char *buf, size_t n) {
    if (!v || CFGetTypeID(v) != CFStringGetTypeID() || !CFStringGetCString(v, buf, (CFIndex)n, kCFStringEncodingUTF8)) snprintf(buf, n, "(none)");
    return buf;
}

static void prop(io_registry_entry_t e, const char *key) {
    CFStringRef k = CFStringCreateWithCString(NULL, key, kCFStringEncodingUTF8);
    CFTypeRef v = IORegistryEntryCreateCFProperty(e, k, NULL, 0);
    CFRelease(k);
    char b[256];
    if (!v) printf("  %s: absent\n", key);
    else if (CFGetTypeID(v) == CFNumberGetTypeID()) printf("  %s: %#lx\n", key, num(v));
    else if (CFGetTypeID(v) == CFStringGetTypeID()) printf("  %s: %s\n", key, str(v, b, sizeof b));
    else if (CFGetTypeID(v) == CFBooleanGetTypeID()) printf("  %s: %s\n", key, v == kCFBooleanTrue ? "yes" : "no");
    else if (CFGetTypeID(v) == CFDataGetTypeID()) printf("  %s: %ld bytes\n", key, (long)CFDataGetLength(v));
    else if (CFGetTypeID(v) == CFArrayGetTypeID()) {
        printf("  %s:", key);
        for (CFIndex i = 0; i < CFArrayGetCount(v); i++) {
            CFDictionaryRef p = CFArrayGetValueAtIndex(v, i);
            printf(" (%#lx, %#lx)", num(CFDictionaryGetValue(p, CFSTR(kIOHIDDeviceUsagePageKey))), num(CFDictionaryGetValue(p, CFSTR(kIOHIDDeviceUsageKey))));
        }
        printf("\n");
    }
    if (v) CFRelease(v);
}

static void entry(io_registry_entry_t e) {
    io_name_t name, cls;
    io_string_t path;
    IORegistryEntryGetName(e, name);
    IOObjectGetClass(e, cls);
    printf("%s (%s)", name, cls);
    if (IORegistryEntryGetPath(e, kIOServicePlane, path) == KERN_SUCCESS) printf(" at %s", path);
    printf("\n");
}

// A class's matching dictionary with a vendor and a product (none for -1).
static CFMutableDictionaryRef vendor_product(const char *cls, const char *vk, const char *pk, int v, int p) {
    CFMutableDictionaryRef d = cls ? IOServiceMatching(cls) : CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFStringRef k1 = CFStringCreateWithCString(NULL, vk, kCFStringEncodingUTF8), k2 = CFStringCreateWithCString(NULL, pk, kCFStringEncodingUTF8);
    CFNumberRef a = CFNumberCreate(NULL, kCFNumberIntType, &v), b = CFNumberCreate(NULL, kCFNumberIntType, &p);
    CFDictionarySetValue(d, k1, a);
    if (p >= 0) CFDictionarySetValue(d, k2, b);
    CFRelease(k1), CFRelease(k2), CFRelease(a), CFRelease(b);
    return d;
}

// -- the registry, as it is --------------------------------------------------------------

static void list_registry(void) {
    io_iterator_t it;
    io_object_t o;
    IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching(kIOHIDDeviceKey), &it);
    while ((o = IOIteratorNext(it))) {
        entry(o);
        const char *keys[] = {kIOHIDVendorIDKey, kIOHIDProductIDKey, kIOHIDProductKey, kIOHIDManufacturerKey, kIOHIDSerialNumberKey, kIOHIDTransportKey,
                              kIOHIDPrimaryUsagePageKey, kIOHIDPrimaryUsageKey, kIOHIDDeviceUsagePairsKey, kIOHIDMaxInputReportSizeKey,
                              kIOHIDMaxOutputReportSizeKey, kIOHIDMaxFeatureReportSizeKey, kIOHIDReportDescriptorKey, NULL};
        for (int i = 0; keys[i]; i++) prop(o, keys[i]);
        printf("  an IOHIDDevice %d, an IOService %d\n", IOObjectConformsTo(o, "IOHIDDevice"), IOObjectConformsTo(o, "IOService"));
        IOObjectRelease(o);
    }
    if (it) IOObjectRelease(it);
    IOServiceGetMatchingServices(kIOMainPortDefault, vendor_product(kIOUSBHostDeviceClassName, kUSBVendorID, kUSBProductID, 0x1209, 0x0002), &it);
    while (it && (o = IOIteratorNext(it))) {
        entry(o);
        const char *keys[] = {kUSBVendorID, kUSBProductID, kUSBDeviceReleaseNumber, "bcdUSB", kUSBDeviceClass, kUSBVendorString, kUSBProductString, kUSBSerialNumberString,
                              "bNumConfigurations", "kUSBCurrentConfiguration", "locationID", NULL};
        for (int i = 0; keys[i]; i++) prop(o, keys[i]);
        printf("  an IOUSBDevice %d\n", IOObjectConformsTo(o, "IOUSBDevice"));
        io_iterator_t kids;
        io_object_t k;
        IORegistryEntryGetChildIterator(o, kIOServicePlane, &kids);
        while ((k = IOIteratorNext(kids))) {
            entry(k);
            const char *ik[] = {kUSBInterfaceNumber, kUSBAlternateSetting, kUSBInterfaceClass, kUSBInterfaceSubClass, kUSBInterfaceProtocol, kUSBNumEndpoints,
                                kUSBConfigurationValue, kUSBVendorID, NULL};
            for (int i = 0; ik[i]; i++) prop(k, ik[i]);
            IOObjectRelease(k);
        }
        IOObjectRelease(kids);
        IOObjectRelease(o);
    }
    if (it) IOObjectRelease(it);
    // (A dictionary of a family's keys and no class: every service of
    // another family, which does not read them, matches it.)
    IOServiceGetMatchingServices(kIOMainPortDefault, vendor_product(NULL, kUSBVendorID, kUSBProductID, 0x1209, 0x0002), &it);
    int n = 0;
    while (it && (o = IOIteratorNext(it))) n++, IOObjectRelease(o);
    printf("more than the USB device without its class %d\n", n > 1);
    if (it) IOObjectRelease(it);
    // A matching dictionary of the HID family's keys, and one of a usage.
    CFMutableDictionaryRef d = IOServiceMatching(kIOHIDDeviceKey);
    int page = 1, usage = 5;
    CFNumberRef pn = CFNumberCreate(NULL, kCFNumberIntType, &page), un = CFNumberCreate(NULL, kCFNumberIntType, &usage);
    CFDictionarySetValue(d, CFSTR(kIOHIDDeviceUsagePageKey), pn);
    CFDictionarySetValue(d, CFSTR(kIOHIDDeviceUsageKey), un);
    printf("a game pad by its usage %d\n", IOServiceGetMatchingService(kIOMainPortDefault, d) != 0);
    d = IOServiceMatching(kIOHIDDeviceKey);
    usage = 6;
    CFNumberRef kb = CFNumberCreate(NULL, kCFNumberIntType, &usage);
    CFDictionarySetValue(d, CFSTR(kIOHIDDeviceUsagePageKey), pn);
    CFDictionarySetValue(d, CFSTR(kIOHIDDeviceUsageKey), kb);
    printf("a keyboard by its usage %d\n", IOServiceGetMatchingService(kIOMainPortDefault, d) != 0);
}

// -- the HID manager and notifications ----------------------------------------------------

static IOHIDManagerRef manager;
static IOHIDDeviceRef pad;
static int step;
static int seen_manager, seen_note, seen_removed, seen_terminated, seen_report, seen_values;
static char values[1024];

static void elements(IOHIDDeviceRef d) {
    CFArrayRef els = IOHIDDeviceCopyMatchingElements(d, NULL, kIOHIDOptionsTypeNone);
    printf("elements: %ld\n", (long)CFArrayGetCount(els));
    for (CFIndex i = 0; i < CFArrayGetCount(els); i++) {
        IOHIDElementRef e = (IOHIDElementRef)CFArrayGetValueAtIndex(els, i);
        printf("  cookie %u type %d page %#x usage %#x report %u size %u count %u logical %ld..%ld%s%s\n", (unsigned)IOHIDElementGetCookie(e), IOHIDElementGetType(e),
               IOHIDElementGetUsagePage(e), IOHIDElementGetUsage(e), IOHIDElementGetReportID(e), IOHIDElementGetReportSize(e), IOHIDElementGetReportCount(e),
               (long)IOHIDElementGetLogicalMin(e), (long)IOHIDElementGetLogicalMax(e), IOHIDElementIsArray(e) ? " array" : "",
               IOHIDElementGetParent(e) ? "" : " (top)");
    }
    CFRelease(els);
    // Those of one usage page.
    CFMutableDictionaryRef m = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    int page = 9;
    CFNumberRef pn = CFNumberCreate(NULL, kCFNumberIntType, &page);
    CFDictionarySetValue(m, CFSTR(kIOHIDElementUsagePageKey), pn);
    els = IOHIDDeviceCopyMatchingElements(d, m, kIOHIDOptionsTypeNone);
    printf("buttons: %ld\n", els ? (long)CFArrayGetCount(els) : 0L);
    if (els) CFRelease(els);
}

static void matched(void *ctx, IOReturn r, void *sender, IOHIDDeviceRef d) {
    char b[128];
    long product = num(IOHIDDeviceGetProperty(d, CFSTR(kIOHIDProductIDKey)));
    if (product == 1 && !pad) {
        pad = d;
        printf("matched %s (%#lx), conforms to a game pad %d, a keyboard %d\n", str(IOHIDDeviceGetProperty(d, CFSTR(kIOHIDProductKey)), b, sizeof b), product,
               IOHIDDeviceConformsTo(d, 1, 5), IOHIDDeviceConformsTo(d, 1, 6));
        elements(d);
        uint8_t feature[8] = {0};
        CFIndex n = sizeof feature;
        IOReturn g = IOHIDDeviceGetReport(d, kIOHIDReportTypeFeature, 3, feature, &n);
        printf("feature report 3: %#x, %ld bytes:", g, (long)n);
        for (CFIndex i = 0; i < n; i++) printf(" %02x", feature[i]);
        printf("\n");
        // (The values the feature report gave its element.)
        CFMutableDictionaryRef m = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        int page = 0xff00;
        CFNumberRef pn = CFNumberCreate(NULL, kCFNumberIntType, &page);
        CFDictionarySetValue(m, CFSTR(kIOHIDElementUsagePageKey), pn);
        CFArrayRef fe = IOHIDDeviceCopyMatchingElements(d, m, 0);
        IOHIDValueRef v = NULL;
        if (fe && IOHIDDeviceGetValue(d, (IOHIDElementRef)CFArrayGetValueAtIndex(fe, 0), &v) == kIOReturnSuccess)
            printf("its element's value %ld\n", (long)IOHIDValueGetIntegerValue(v));
        step = 1;
        uint8_t out[2] = {2, 0x05};
        printf("output report 2: %#x\n", IOHIDDeviceSetReport(d, kIOHIDReportTypeOutput, 2, out, 2));
        return;
    }
    if (product == 3) seen_manager++;
}

static void removed(void *ctx, IOReturn r, void *sender, IOHIDDeviceRef d) {
    if (num(IOHIDDeviceGetProperty(d, CFSTR(kIOHIDProductIDKey))) == 3) seen_removed++;
}

static void report(void *ctx, IOReturn r, void *sender, IOHIDReportType type, uint32_t id, uint8_t *bytes, CFIndex n) {
    printf("input report %u (%d), %ld bytes:", id, type, (long)n);
    for (CFIndex i = 0; i < n; i++) printf(" %02x", bytes[i]);
    printf("\n");
    seen_report++;
}

static void value(void *ctx, IOReturn r, void *sender, IOHIDValueRef v) {
    IOHIDElementRef e = IOHIDValueGetElement(v);
    size_t at = strlen(values);
    snprintf(values + at, sizeof values - at, "  page %#x usage %#x value %ld\n", IOHIDElementGetUsagePage(e), IOHIDElementGetUsage(e), (long)IOHIDValueGetIntegerValue(v));
    seen_values++;
}

static void first_match(void *ctx, io_iterator_t it) {
    io_object_t o;
    while ((o = IOIteratorNext(it))) {
        CFTypeRef p = IORegistryEntryCreateCFProperty(o, CFSTR(kIOHIDProductIDKey), NULL, 0);
        if (num(p) == 3) seen_note++;
        if (p) CFRelease(p);
        IOObjectRelease(o);
    }
}

static void terminated(void *ctx, io_iterator_t it) {
    io_object_t o;
    while ((o = IOIteratorNext(it))) {
        CFTypeRef p = IORegistryEntryCreateCFProperty(o, CFSTR(kIOHIDProductIDKey), NULL, 0);
        if (num(p) == 3) seen_terminated++;
        if (p) CFRelease(p);
        IOObjectRelease(o);
    }
}

// Each turn of the run loop: whether a step's events have all come.
static void tick(CFRunLoopTimerRef t, void *info) {
    if (step == 1 && seen_report && seen_values >= 3 && seen_manager && seen_note) {
        printf("its values:\n%s", values);
        printf("the second pad: matched by the manager %d, first matched %d\n", seen_manager, seen_note);
        CFSetRef devs = IOHIDManagerCopyDevices(manager);
        printf("the manager's devices: %ld\n", devs ? (long)CFSetGetCount(devs) : 0L);
        if (devs) CFRelease(devs);
        step = 2;
        uint8_t out[2] = {2, 0x06};
        printf("output report 2: %#x\n", IOHIDDeviceSetReport(pad, kIOHIDReportTypeOutput, 2, out, 2));
    } else if (step == 2 && seen_removed && seen_terminated) {
        printf("the second pad: removed %d, terminated %d\n", seen_removed, seen_terminated);
        CFSetRef devs = IOHIDManagerCopyDevices(manager);
        printf("the manager's devices: %ld\n", devs ? (long)CFSetGetCount(devs) : 0L);
        if (devs) CFRelease(devs);
        printf("closed %#x\n", IOHIDManagerClose(manager, 0));
        exit(0);
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    alarm(120);
    list_registry();

    IONotificationPortRef port = IONotificationPortCreate(kIOMainPortDefault);
    CFRunLoopAddSource(CFRunLoopGetMain(), IONotificationPortGetRunLoopSource(port), kCFRunLoopDefaultMode);
    io_iterator_t first, gone;
    IOServiceAddMatchingNotification(port, kIOFirstMatchNotification, vendor_product(kIOHIDDeviceKey, kIOHIDVendorIDKey, kIOHIDProductIDKey, 0x1209, -1), first_match, NULL, &first);
    int there = 0;
    io_object_t o;
    while ((o = IOIteratorNext(first))) there++, IOObjectRelease(o);
    printf("first matched already: %d\n", there);
    IOServiceAddMatchingNotification(port, kIOTerminatedNotification, vendor_product(kIOHIDDeviceKey, kIOHIDVendorIDKey, kIOHIDProductIDKey, 0x1209, -1), terminated, NULL, &gone);
    while ((o = IOIteratorNext(gone))) IOObjectRelease(o);

    manager = IOHIDManagerCreate(NULL, kIOHIDOptionsTypeNone);
    CFDictionaryRef two[] = {vendor_product(NULL, kIOHIDVendorIDKey, kIOHIDProductIDKey, 0x1209, 0x0001), vendor_product(NULL, kIOHIDVendorIDKey, kIOHIDProductIDKey, 0x1209, 0x0003)};
    CFArrayRef multiple = CFArrayCreate(NULL, (const void **)two, 2, &kCFTypeArrayCallBacks);
    IOHIDManagerSetDeviceMatchingMultiple(manager, multiple);
    CFSetRef devs = IOHIDManagerCopyDevices(manager);
    printf("the manager's devices: %ld\n", devs ? (long)CFSetGetCount(devs) : 0L);
    IOHIDManagerRegisterDeviceMatchingCallback(manager, matched, NULL);
    IOHIDManagerRegisterDeviceRemovalCallback(manager, removed, NULL);
    IOHIDManagerRegisterInputReportCallback(manager, report, NULL);
    IOHIDManagerRegisterInputValueCallback(manager, value, NULL);
    IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
    printf("opened %#x\n", IOHIDManagerOpen(manager, kIOHIDOptionsTypeNone));

    CFRunLoopTimerRef t = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.05, 0.05, 0, 0, tick, NULL);
    CFRunLoopAddTimer(CFRunLoopGetMain(), t, kCFRunLoopDefaultMode);
    CFRunLoopRun();
    return 1;
}
