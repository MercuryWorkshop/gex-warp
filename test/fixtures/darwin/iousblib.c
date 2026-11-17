// darwin/devices-page's third program: IOUSBLib's plug-in interfaces over
// the page's `devices` service, the way libusb's darwin backend uses them,
// built against the SDK's IOKit. It finds the page's USB device among the
// IOUSBDevice services, makes its plug-in and asks it for a device
// interface, opens it, reads its fields and configuration descriptor and
// makes a vendor request; finds its interface through the interface
// iterator, opens it, reads each pipe's properties, writes to the bulk
// pipe out and reads back (waited for, then asynchronously through the
// interface's event source on the run loop), hears the interrupt pipe
// stall and clears it; and lets everything go.
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOCFPlugIn.h>
#include <IOKit/IOKitLib.h>
#include <IOKit/usb/IOUSBLib.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static IOUSBInterfaceInterface700 **intf;
static int pending;
static char got[32];

static void wrote(void *refcon, IOReturn r, void *arg0) {
    printf("async write: %#x, %lu bytes\n", r, (unsigned long)(uintptr_t)arg0);
    if (!--pending) CFRunLoopStop(CFRunLoopGetCurrent());
}

static void read_back(void *refcon, IOReturn r, void *arg0) {
    printf("async read: %#x, %lu bytes: %.*s\n", r, (unsigned long)(uintptr_t)arg0, (int)(uintptr_t)arg0, got);
    if (!--pending) CFRunLoopStop(CFRunLoopGetCurrent());
}

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    alarm(120);
    CFMutableDictionaryRef m = IOServiceMatching(kIOUSBDeviceClassName);
    int v = 0x1209, p = 2;
    CFNumberRef vn = CFNumberCreate(NULL, kCFNumberIntType, &v), pn = CFNumberCreate(NULL, kCFNumberIntType, &p);
    CFDictionarySetValue(m, CFSTR(kUSBVendorID), vn);
    CFDictionarySetValue(m, CFSTR(kUSBProductID), pn);
    io_iterator_t it;
    IOServiceGetMatchingServices(kIOMainPortDefault, m, &it);
    io_service_t s = IOIteratorNext(it);
    printf("an IOUSBDevice %d\n", s != 0);
    CFNumberRef addr = IORegistryEntryCreateCFProperty(s, CFSTR("USB Address"), NULL, 0), session = IORegistryEntryCreateCFProperty(s, CFSTR("sessionID"), NULL, 0);
    printf("its address and session %d %d\n", addr != NULL, session != NULL);

    IOCFPlugInInterface **plugin = NULL;
    SInt32 score;
    printf("an interface's plug-in of it %#x\n", IOCreatePlugInInterfaceForService(s, kIOUSBInterfaceUserClientTypeID, kIOCFPlugInInterfaceID, &plugin, &score));
    IOReturn r = IOCreatePlugInInterfaceForService(s, kIOUSBDeviceUserClientTypeID, kIOCFPlugInInterfaceID, &plugin, &score);
    printf("its plug-in %#x\n", r);
    IOUSBDeviceInterface650 **dev = NULL;
    HRESULT h = (*plugin)->QueryInterface(plugin, CFUUIDGetUUIDBytes(kIOUSBDeviceInterfaceID650), (LPVOID *)&dev);
    IOUSBInterfaceInterface700 **wrong = NULL;
    HRESULT h2 = (*plugin)->QueryInterface(plugin, CFUUIDGetUUIDBytes(kIOUSBInterfaceInterfaceID700), (LPVOID *)&wrong);
    printf("a device interface %d, not an interface's %d\n", h == S_OK && dev, h2 != S_OK && !wrong);
    IODestroyPlugInInterface(plugin);
    printf("opened %#x, again %#x\n", (*dev)->USBDeviceOpenSeize(dev), (*dev)->USBDeviceOpen(dev));
    UInt8 cls, sub, proto, speed, configs, config, msi, psi, snsi;
    UInt16 vendor, product, release;
    UInt32 location;
    (*dev)->GetDeviceClass(dev, &cls);
    (*dev)->GetDeviceSubClass(dev, &sub);
    (*dev)->GetDeviceProtocol(dev, &proto);
    (*dev)->GetDeviceVendor(dev, &vendor);
    (*dev)->GetDeviceProduct(dev, &product);
    (*dev)->GetDeviceReleaseNumber(dev, &release);
    (*dev)->GetDeviceSpeed(dev, &speed);
    (*dev)->GetNumberOfConfigurations(dev, &configs);
    (*dev)->GetConfiguration(dev, &config);
    (*dev)->GetLocationID(dev, &location);
    (*dev)->USBGetManufacturerStringIndex(dev, &msi);
    (*dev)->USBGetProductStringIndex(dev, &psi);
    (*dev)->USBGetSerialNumberStringIndex(dev, &snsi);
    printf("class %d/%d/%d vendor %#x product %#x release %#x speed %d configurations %d, in %d, strings %d %d %d, located %d\n", cls, sub, proto, vendor, product, release,
           speed, configs, config, msi, psi, snsi, location != 0);
    IOUSBConfigurationDescriptorPtr cd = NULL;
    printf("configuration descriptor %#x", (*dev)->GetConfigurationDescriptorPtr(dev, 0, &cd));
    printf(": %d bytes, value %d, %d interfaces\n", USBToHostWord(cd->wTotalLength), cd->bConfigurationValue, cd->bNumInterfaces);
    printf("no second %#x\n", (*dev)->GetConfigurationDescriptorPtr(dev, 1, &cd));
    char buf[16] = {0};
    IOUSBDevRequestTO req = {USBmakebmRequestType(kUSBIn, kUSBVendor, kUSBDevice), 1, 0x1234, 0, sizeof buf, buf, 0, 1000, 1000};
    printf("vendor request %#x", (*dev)->DeviceRequestTO(dev, &req));
    printf(", %u bytes: %.*s\n", (unsigned)req.wLenDone, (int)req.wLenDone, buf);
    printf("configured %#x\n", (*dev)->SetConfiguration(dev, 1));

    IOUSBFindInterfaceRequest find = {kIOUSBFindInterfaceDontCare, kIOUSBFindInterfaceDontCare, kIOUSBFindInterfaceDontCare, kIOUSBFindInterfaceDontCare};
    io_iterator_t ifs;
    printf("interfaces %#x", (*dev)->CreateInterfaceIterator(dev, &find, &ifs));
    io_service_t is = IOIteratorNext(ifs);
    printf(", one %d, more %d\n", is != 0, IOIteratorNext(ifs) != 0);
    find.bInterfaceClass = 3;
    io_iterator_t none;
    (*dev)->CreateInterfaceIterator(dev, &find, &none);
    printf("of the HID class %d\n", IOIteratorNext(none) != 0);
    r = IOCreatePlugInInterfaceForService(is, kIOUSBInterfaceUserClientTypeID, kIOCFPlugInInterfaceID, &plugin, &score);
    h = (*plugin)->QueryInterface(plugin, CFUUIDGetUUIDBytes(kIOUSBInterfaceInterfaceID700), (LPVOID *)&intf);
    IODestroyPlugInInterface(plugin);
    printf("its interface %#x %d\n", r, h == S_OK);
    printf("opened %#x\n", (*intf)->USBInterfaceOpen(intf));
    UInt8 n, num, alt, iclass;
    (*intf)->GetNumEndpoints(intf, &n);
    (*intf)->GetInterfaceNumber(intf, &num);
    (*intf)->GetAlternateSetting(intf, &alt);
    (*intf)->GetInterfaceClass(intf, &iclass);
    printf("interface %d alternate %d class %#x, %d endpoints\n", num, alt, iclass, n);
    UInt8 out = 0, in = 0, irq = 0;
    for (UInt8 pipe = 0; pipe <= n + 1; pipe++) {
        UInt8 dir, number, type, interval;
        UInt16 size;
        IOReturn e = (*intf)->GetPipeProperties(intf, pipe, &dir, &number, &type, &size, &interval);
        printf("  pipe %d: %#x", pipe, e);
        if (e == kIOReturnSuccess) printf(" direction %d number %d type %d size %d interval %d", dir, number, type, size, interval);
        printf("\n");
        if (e == kIOReturnSuccess && type == kUSBBulk && dir == kUSBOut) out = pipe;
        if (e == kIOReturnSuccess && type == kUSBBulk && dir == kUSBIn) in = pipe;
        if (e == kIOReturnSuccess && type == kUSBInterrupt) irq = pipe;
    }
    printf("written %#x\n", (*intf)->WritePipeTO(intf, out, "hello!", 6, 1000, 1000));
    char back[32] = {0};
    UInt32 size = sizeof back;
    printf("read %#x", (*intf)->ReadPipeTO(intf, in, back, &size, 1000, 1000));
    printf(", %u bytes: %.*s\n", (unsigned)size, (int)size, back);
    printf("written the wrong way %#x\n", (*intf)->WritePipe(intf, in, "x", 1));
    printf("an async read before its source %#x\n", (*intf)->ReadPipeAsync(intf, in, got, sizeof got, read_back, NULL));
    CFRunLoopSourceRef src;
    printf("its event source %#x\n", (*intf)->CreateInterfaceAsyncEventSource(intf, &src));
    CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopDefaultMode);
    pending = 1;
    (*intf)->WritePipeAsync(intf, out, "async", 5, wrote, NULL);
    CFRunLoopRun();
    pending = 1;
    (*intf)->ReadPipeAsync(intf, in, got, sizeof got, read_back, NULL);
    CFRunLoopRun();
    char x[8];
    size = sizeof x;
    printf("interrupt read %#x\n", (*intf)->ReadPipe(intf, irq, x, &size));
    printf("stall cleared %#x\n", (*intf)->ClearPipeStallBothEnds(intf, irq));
    printf("isochronous %#x\n", (*intf)->ReadIsochPipeAsync(intf, in, x, 0, 1, NULL, NULL, NULL));
    printf("closed %#x %#x\n", (*intf)->USBInterfaceClose(intf), (*dev)->USBDeviceClose(dev));
    printf("closed again %#x\n", (*dev)->USBDeviceClose(dev));
    (*intf)->Release(intf);
    (*dev)->Release(dev);
    return 0;
}
